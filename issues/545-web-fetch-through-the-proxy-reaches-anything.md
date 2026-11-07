# 545 — A page fetched through a proxy may lie inside the network

**Status:** open
**Type:** feature
**Priority:** medium
**Area:** Tools and approvals
**Summary:** `web_fetch` refuses addresses inside the network only where it connects itself; a fetch through the proxy the environment names reaches whatever the proxy reaches, internal hosts and the cloud's metadata service among them.

## The problem

Issue [544](544-web-access-tools.md) gives agents `web_fetch`, which fetches a page by a URL the
model writes. A URL can name a host inside the network the server runs in - the server's own REST
API, an intranet service, the cloud's metadata service (`169.254.169.254`) - and the model may
write it because a page it read told it to (server-side request forgery). `web_fetch` refuses
loopback, private and link-local addresses where it connects itself: an address written in the URL
always, a name by the address it resolves to, on every redirect.

But the server reaches the internet through the proxy its environment names
(`HTTPS_PROXY`, decided in 544). Through a proxy, the proxy resolves the name and connects; what
`web_fetch` checks is only the proxy's address. A name whose address lies inside the network but is
not excepted by `NO_PROXY` - or a public name that resolves to an internal address - is fetched by
the proxy, and the agent reads what it answers. What is excepted by `NO_PROXY` is connected to
directly, and so checked.

## Why it matters

- What is refused should not depend on how the network is laid out: whether a host is excepted
  from the proxy is a decision about routing, not about what an agent may read.
- The proxy's own rules are not Caelumora's to know or to rely on; a proxy that lets the server
  reach internal hosts - which a server's proxy often does - lets every agent with `web_fetch`
  reach them too.

## Considerations

- **Resolve before sending.** Resolve the name itself, refuse an internal address, and only then
  hand the request to the proxy. The proxy resolves again and may get another answer (DNS
  rebinding, split-horizon DNS where the proxy sees other names than the server); it narrows the
  gap without closing it.
- **Connect to the address checked, through the proxy.** Send the proxy the address instead of
  the name (`CONNECT 203.0.113.5:443`) and keep the name for TLS (SNI and the certificate) and for
  the `Host` header. Closes the gap; needs a connector of its own below `reqwest`, which does not
  offer it, and proxies that refuse a `CONNECT` to an address exist.
- **The proxy's rules.** Document that the proxy has to refuse internal destinations for the
  server, and leave the check to it. Nothing to build, but the guard is outside Caelumora and
  unseen by it.
- **A proxy of `web_fetch`'s own**, configured on the component instead of read from the
  environment, so that an operator can give agents a proxy that reaches only the internet, apart
  from the one the server uses for its own services. A parameter, so only with a user.

## Done when

- A page that resolves to an address inside the network is not fetched, whether through the proxy
  or not - or the documented rule says why the proxy's rules are relied on, and what they have to
  refuse.

## History

- **2026-11-07** — Opened while planning issue 544: the server has to use the proxy of its
  environment, and the check of addresses for fetches through it was deferred to this issue.
