# 402 — Login attempts are not limited

**Status:** open
**Type:** feature
**Priority:** high
**Area:** Identity and access management
**Summary:** `/auth/login` takes any number of attempts from anyone, so passwords can be guessed without limit, and every attempt costs the server an Argon2 verification that a flood of requests turns into exhausted memory and CPU.

## The problem

`POST /auth/login` is the one route of the REST API that needs no session, and it answers every
request: there is no limit per client, per username or overall. [10](10-user-authentication.md)
left this deliberately as a known limit while the only user provider was `accept_any`, where there
was nothing to guess.

Since [25](25-file-user-backend.md), users have real passwords, and two things follow:

- **Guessing.** A password of the `file` provider can be guessed as fast as the server answers. That
  a wrong password and an unknown name are answered alike ([10](10-user-authentication.md)) keeps
  the names hidden, not the passwords.
- **Load.** Every attempt for a name the file knows - and, so that the time does not tell which
  names exist, every attempt for one it does not - verifies an Argon2 hash: with the default
  parameters about 19 MiB of memory and tens of milliseconds of CPU on a blocking thread. Many
  attempts at once exhaust the blocking pool and the memory, without any valid credential. A
  directory provider ([27](27-ldap-user-backend.md)) moves the cost to the directory, and with it
  the risk of locking accounts there.

## Considerations

- **What is limited.** Per client address alone is defeated by many addresses and wrong behind a
  proxy, where every request comes from the proxy; per username alone lets anyone lock a user out
  by guessing their name. The usual answer is both: a limit per address and a slower one per
  username, the latter delaying rather than locking. Behind a reverse proxy the address comes from
  a header the proxy sets, which must only be believed from a configured proxy.
- **Bounding the cost regardless of who asks.** Independent of the limits, the number of password
  verifications running at once can be bounded (a semaphore around the verification, or a
  concurrency limit on the route), so that a flood is answered with `429`/`503` instead of taking
  the memory. This is the cheaper part and protects even when the limits are misconfigured.
- **Where it lives.** The REST server is where requests and addresses are known; the `UserManager`
  is where attempts per username are known, also for frontends other than the REST server. Which
  owns which limit is to be decided, not assumed - a limit per username in the REST server would
  have to be built again for every other frontend that takes a password.
- **State across processes.** Counters in memory are per process; with several instances
  ([21](21-scaling.md)) an attacker spreads attempts over them. Per process is a start, as long as
  the limit is written down as such.
- **Crate.** `tower_governor` (rate limits as a tower layer, keyed by address) would fit axum; a
  limit per username needs its own small counter.

## Done when

- Login attempts are limited per client and per username, with the limits configurable and the
  behaviour behind a proxy decided.
- The number of password verifications running at once is bounded, and an attempt beyond it is
  refused without verifying anything.
- A refused attempt is answered with problem details (`429` with `Retry-After`) and logged without
  the password.
- `docs/REST-SERVER.md` and `docs/SECURITY.md` say what is limited where.

## History

- **2026-10-17** — Opened when [10](10-user-authentication.md) was closed: its known limit "no rate
  limiting on `/auth/login`" became urgent with the password hashes of
  [25](25-file-user-backend.md).
