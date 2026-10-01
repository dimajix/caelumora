// What the channel's moderator means for the user: who is asked to respond
// to a message (issue 44). That is decided by the policy in the moderator's
// slot (issue 174), so it is the policy this explains.

interface ResponderPolicyInfo {
  label: string;
  description: string;
}

const RESPONDER_POLICIES: Record<string, ResponderPolicyInfo> = {
  broadcast_policy: {label: 'Everyone', description: 'Every agent answers every message.'},
  mentions_only_policy: {label: 'Mentions only', description: 'Agents answer only when you @-mention them.'},
  random_policy: {label: 'Random', description: 'One agent, chosen at random, answers each message.'},
  round_robin_policy: {label: 'Round robin', description: 'The agents take turns answering.'},
};

/** The policy kinds this UI knows, for choosing one (issue 09). */
export const RESPONDER_POLICY_KINDS = Object.keys(RESPONDER_POLICIES);

/** The policy of a channel that names none - the server's default. */
export const DEFAULT_RESPONDER_POLICY = 'round_robin_policy';

/**
 * The label and explanation of a policy kind. A kind this UI does not know
 * is shown by its name; `null` is a preconfigured policy, whose kind the
 * server does not tell. In every channel, @-mentioned agents answer instead
 * of those the policy would choose.
 */
export function responderPolicyInfo(kind: string | null | undefined): ResponderPolicyInfo {
  if (!kind) {
    return {label: 'Preconfigured', description: 'A preconfigured moderator decides who answers.'};
  }
  return RESPONDER_POLICIES[kind] ?? {label: kind, description: `Responder policy "${kind}".`};
}

/** Whether agents in a channel with this policy answer only when mentioned. */
export function answersOnlyWhenMentioned(kind: string | null | undefined): boolean {
  return kind === 'mentions_only_policy';
}
