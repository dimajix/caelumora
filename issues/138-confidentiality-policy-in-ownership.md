# 138 — The confidentiality rules are part of the ownership policy

**Status:** done
**Area:** Identity and access management

## The problem

`OwnershipSecurityManager` answers two unrelated kinds of question. Most of its rules are about
ownership: the owner may change and delete, contributors may read, nobody acts in another's name.
But it also decides by classification - whether one channel may read another
(`ActionType::ReadChannel`) and, with [133](133-classification-model.md), whether a classification
may be given at all - by applying the flow rule of `caelumora_model::confidentiality` with the
compartments the configuration defines.

The confidentiality rules live there only because it is the one real policy. Issue 133 calls
ownership and classification separate axes; the policy mixes them.

## Why it matters

- A second policy - Open Policy Agent ([24](24-opa-security-manager.md)), or any deployment's own -
  would have to re-implement the flow rule, or leave it out and open every channel to every other.
- `allow_all` (`DummySecurityManager`) ignores classification entirely, so a development setup
  cannot be used to try out compartments.
- The compartments are configured as a section of their own (`AppConfig`'s `compartments`), not as a
  parameter of a policy, yet only the ownership policy is handed them.

## Considerations

- **A confidentiality part every policy uses.** The `SecurityManager` is composed of the ownership
  (or another) policy and a confidentiality policy that holds the compartments and answers
  `ReadChannel` and classification questions; the ownership policy no longer knows compartments.
  Where the composition happens - a wrapping `SecurityManager`, or
  `AppConfig::create_security_manager` - is open.
- **Leave it to the policies.** Each policy decides everything; OPA would receive the
  classifications and the compartments in its input document. Keeps the `SecurityManager` one seam,
  at the price of every policy having to get the flow rule right.
- Whether `allow_all` should still allow every flow is part of the decision.

## Done when

- The ownership policy decides by ownership alone.
- Classification questions are answered by one piece of code, whichever policy is configured, or it
  is decided and recorded why not.

## History

- **2026-09-30** — Opened while implementing step 1 of [133](133-classification-model.md): the
  compartments became a top-level section of `AppConfig`, handed to the `OwnershipSecurityManager`
  because it is the policy that applies the confidentiality rules today. Separating those rules out
  was deliberately left for later.
- **2026-09-30** — The ownership policy now also holds the roles and decides clearances
  (`JoinChannel`, step 5 of [133](133-classification-model.md)); they belong to the same
  confidentiality part.
- **2026-10-01** — Decided: every policy decides everything, confidentiality included ("Leave it to
  the policies"). Confidentiality and ownership are not always cleanly separable - a superuser who
  may act against the rules, as `ActorId::System` already does, is a decision about both - so a
  composed confidentiality part that a policy can only add to was rejected, as was a seam of its own
  beside the `SecurityManager`. Instead the rules are one building block, `ConfidentialityRules` in
  `caelumora_model::confidentiality`: the flow rule, a session's consent (issue 134), which
  compartments may be given, and the clearances of the configured roles. `OwnershipSecurityManager`
  holds it in place of compartments and roles (`with_rules` replaces `with_compartments` and
  `with_roles`) and adds only its own part - who acts, who may reclassify what. A policy outside the
  binary, such as OPA, states its own rules; `allow_all` stays without any. So the first point of
  *Done when* is met only in substance: the ownership policy still applies the rules, but no longer
  holds them itself; the second is met by this decision.
- **2026-10-11** — Reversed by step 2b of issue [304](304-review-of-the-permission-concept.md):
  the flow decisions left the `SecurityManager` for a `ConfidentialityPolicy` of their own, asked
  beside it, and the ownership policy no longer applies the rules. A superuser who may act against
  both needs a hook in both, which was accepted. The text above stays as the record of what was
  decided then.
