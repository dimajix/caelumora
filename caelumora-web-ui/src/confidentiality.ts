// What a channel's confidentiality level means for the user: how far its
// contents may travel (issue 61).

/** The levels a channel may have; `private` is an actor's, never a channel's. */
export type ChannelConfidentiality = 'public' | 'session' | 'channel';

interface ConfidentialityInfo {
  label: string;
  description: string;
}

const LEVELS: Record<ChannelConfidentiality, ConfidentialityInfo> = {
  public: {label: 'Public', description: 'Any channel may read what is said here, beyond this session too.'},
  session: {label: 'Session', description: 'The other channels of this session may read what is said here.'},
  channel: {label: 'Channel', description: 'What is said here stays in this channel; no other channel may read it.'},
};

/** The levels, from the most open, for choosing one. */
export const CONFIDENTIALITY_LEVELS = Object.keys(LEVELS) as ChannelConfidentiality[];

/** The level of a channel whose creator names none - the server's default. */
export const DEFAULT_CONFIDENTIALITY: ChannelConfidentiality = 'session';

/** The label and explanation of a level; one this UI does not know is shown by its name. */
export function confidentialityInfo(level: string): ConfidentialityInfo {
  return LEVELS[level as ChannelConfidentiality] ?? {label: level, description: `Confidentiality "${level}".`};
}
