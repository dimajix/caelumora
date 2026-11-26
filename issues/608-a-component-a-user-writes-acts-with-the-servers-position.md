# 608 — A component a user writes acts with the server's position

**Status:** open
**Type:** question
**Priority:** medium
**Area:** Identity and access management
**Summary:** Whoever may write a component decides what the server does with its own position - which hosts it sends requests to from inside its network, which files it reads with its own rights - and nothing but the absence of routes keeps users from writing components.

## The problem

A component is configured by whoever writes it, and built and run by the server. What its params
name, the server does from where it stands and with the rights it has:

- **A URL** - an `mcp_server`'s, an `a2a_agent`'s, an `openapi_server`'s (issue
  [252](252-openapi-tool.md)) `endpoint` - is reached from inside the server's network: internal
  services, admin interfaces, a cloud's metadata service, which often hands out credentials. What
  answers comes back to whoever asked, as a tool's result. An `openapi_server` is the strongest
  case: whoever writes its document decides arbitrary requests and is given the answers. A proxy
  helps only where it is taken: a host the environment's `no_proxy` names is reached directly.
- **A file** - the document of an `openapi_server` named by `spec_file` (issue 252) - is read with
  the server's rights: `local/secrets.json`, the vaults of the agents' credentials, the
  configuration, the data of the JSON store. What of it reaches the writer is narrower - a document
  that is no OpenAPI is refused, and its errors name no content -, but that a file exists, and
  whether it is JSON, is told.
- **A secret** - `auth.secret` - is used as its vault grants and sent only where its origins allow
  (issue [310](310-secrets-have-no-owner.md)): of the three, the one already decided.

Today no route lets a user write a personality or a component bundle; the one configuration a
request writes is a channel's, which takes a `Moderator` and what that needs. Users take what the
operator and the seed prepared. That is why `operator_only` could go (issue
[551](551-a-decision-model-is-asked-past-the-model-plugin.md)): `mcp_server`, with its `endpoint`,
was checked and needed no guard. It holds as long as no such route comes - and nothing says so but
the routes that are not there.

## Why it matters

- **Users writing their own agents** - personalities, bundles, tools of their own - is the obvious
  next step, and issue [343](343-users-bring-their-own-secrets.md) prepares it for secrets. Each
  URL and file a component may name opens with it.
- **A guard in each component** would be the wrong layer: the URL of `web_fetch` is the model's,
  checked where it connects (issue [545](545-web-fetch-through-the-proxy-reaches-anything.md)), but
  an endpoint or a file is the writer's, and every kind that reaches something would need one of
  its own.

## Considerations

- **What a user may write is decided where it is written.** The registry already asks of a
  configuration as it is written, in the writer's name: what a container accepts (issue 337), the
  `ref`s it names (issue 321), the secrets it uses (issue 310). Whether a writer may name an
  endpoint or a file would be decided there too, by the kind's declaration - as `reaches` declares
  the `endpoint` (issue 310) -, not by the component.
- **Options:**
  - Users may only take prepared components - bundles the operator or someone trusted wrote - and
    choose among them, never write params that reach anything. Simple, and what holds today by
    the absence of routes; it would hold by a rule instead.
  - A kind declares what of its params acts with the server's position (an endpoint, a file), and
    only whoever is granted that may write them - a role, as an operator's.
  - Endpoints a user may name are bound like the origins of a vault: a list the operator keeps of
    the hosts users' components may reach, beside the proxy.
- **Files** are likely the operator's alone in any option: a user has no files on the server to
  name, and a document a user brings would be content of their own, kept as one, not a path.

## Done when

- It is decided who may write a component whose params name what the server reaches or reads, and
  the registry refuses anyone else where the configuration is written - before a route lets users
  write personalities or component bundles.

## History

- **2026-11-26** — Opened at the maintainer's request while deciding `spec_file` for issue
  [252](252-openapi-tool.md): a file named in a component's params is read with the server's rights,
  as a URL is reached from its network; both are to be decided on the layer of who may write a
  component, not in each component.
