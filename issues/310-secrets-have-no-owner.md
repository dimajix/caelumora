# 310 — Any secret can be named by whoever writes a component

**Status:** open
**Area:** Identity and access management

## The problem

A component that talks to a remote system names its credential by a key, in its endpoint
(`RemoteEndpoint::auth`, for instance `auth.secret` of an `mcp_tool` or of an external agent). The
key is resolved through the `SecretManager` on every use, in the context of whoever acts - for an
agent its owner (issues [12](12-user-and-agent-permissions.md),
[255](255-whose-name-agents-act-in.md)). `SecretManager::resolve` asks the policy
`ActionType::ReadSecret(key)`, and the ownership policy always answers yes.

So nothing decides *which* secrets someone may use. Issue 12 settled *whose* a credential is - the
agent's owner's -, not whether that owner may name it. A secret carries no `SecurityAttributes`: it
is a key and a value, with nobody owning it and nobody granted it. Whoever may write a personality
or a preconfigured component could name any key the providers know - the operator's model API key,
another team's token - and point the endpoint at a URL of their choosing; the agent would send the
credential there on its first call.

The gap is latent today: personalities and components are written only by the seed and the
configuration, by whoever runs the server, and no REST route lets a user write them. The ownership
policy would already allow a user to create a personality of their own (`CreatePersonality`); the
gap opens as soon as anything lets them.

## Why it matters

- **Credentials leave the system.** Unlike content, which the flow rules keep inside, a secret is
  handed to a remote side by design; the only protection is who may name which secret where.
- **Users writing their own agents** is the obvious next step for personalities and preconfigured
  components, and it must not wait for this to be noticed then.
- **An external policy (issue [24](24-opa-security-manager.md))** cannot decide `ReadSecret` well
  either: it is given a key, not an object with an owner and an access list.

## Considerations

- **Secrets as entities with `SecurityAttributes`**: an owner and an access list, as every other
  entity has, so that "who may use this credential" is a grant (`Use`) like any other. Where they
  are kept - beside the providers' values, or as a registry of keys the configuration declares -
  is to be decided; the values stay with the providers.
- **When to ask**: when a component naming a secret is written (whoever writes it may use the
  secret), and when the secret is resolved (the principal may use it). The first refuses early and
  where the decision is made; the second holds if grants change afterwards. Probably both, by the
  pattern of issue 304: the act that names it asks, and the use asks as the operation it is.
- **What a secret may be sent to**: a grant could be bound to the endpoints it may be used with, so
  that a usable secret still cannot be pointed elsewhere.
- **The system's secrets**: those the configuration names for the server itself (a storage of
  workspaces, the model of the system) are the system's, used in its context.

## Done when

- A secret has an owner and says who may use it; `ReadSecret` is decided by it and not always
  allowed.
- Naming a secret in a component that someone writes is refused unless they may use it.
- The security suite shows a user who cannot make an agent use a secret that is not theirs.

## History

- **2026-10-11** — Opened while reviewing the permission concept (issue
  [304](304-review-of-the-permission-concept.md), F12): of the four actions the policy always
  allows, `ReadSecret` is the one whose "yes" hides a gap, since secrets have no owner to decide by.
  304 keeps `ReadSecret` as it is and documents that only the seed and the configuration name
  secrets until this is done.
