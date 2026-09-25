// What the channel's moderator means for the user: who is asked to respond
// to a message (issue 44).

interface ModeratorInfo {
  label: string;
  description: string;
}

const MODERATORS: Record<string, ModeratorInfo> = {
  broadcast: {label: 'Everyone', description: 'Every agent answers every message.'},
  mentions_only: {label: 'Mentions only', description: 'Agents answer only when you @-mention them.'},
  random: {label: 'Random', description: 'One agent, chosen at random, answers each message.'},
  round_robin: {label: 'Round robin', description: 'The agents take turns answering.'},
};

/** The moderator kinds this UI knows, for choosing one (issue 09). */
export const MODERATOR_KINDS = Object.keys(MODERATORS);

/** The moderator of a channel that names none - the server's default. */
export const DEFAULT_MODERATOR = 'round_robin';

/**
 * The label and explanation of a moderator kind. A kind this UI does not
 * know is shown by its name. In every channel, @-mentioned agents answer
 * instead of those the moderator would choose.
 */
export function moderatorInfo(kind: string): ModeratorInfo {
  return MODERATORS[kind] ?? {label: kind, description: `Moderator "${kind}".`};
}

/** Whether agents in a channel with this moderator answer only when mentioned. */
export function answersOnlyWhenMentioned(kind: string | undefined): boolean {
  return kind === 'mentions_only';
}
