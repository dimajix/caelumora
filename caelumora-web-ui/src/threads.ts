// How a thread of a channel is told (issue 595): by whose sub-agent's it is and who takes part in
// it - at the message it hangs at, in the channel's panel, and at the head of its pane.

import type {ChannelParticipant, ChannelThread} from './types';
import {sameActor} from './actor';

/** A thread as it is told: its title, and who takes part in it. */
export interface ThreadTold {
  /** `Thread of` the agent it was delegated to - a sub-agent's -, else `Thread`. */
  title: string;
  takingPart: ChannelParticipant[];
  /** Whether one taking part answers in it now. */
  answering: boolean;
}

/** `thread` as it is told, among `participants`. */
export function describeThread(thread: ChannelThread, participants: ChannelParticipant[]): ThreadTold {
  const recipient = thread.recipient;
  const delegatedTo = recipient ? participants.find(m => sameActor(m.id, recipient))?.name : undefined;
  const takingPart = participants.filter(m => m.threads?.includes(thread.id));
  return {
    title: delegatedTo ? `Thread of ${delegatedTo}` : 'Thread',
    takingPart,
    answering: takingPart.some(m => m.answering_in === thread.id),
  };
}
