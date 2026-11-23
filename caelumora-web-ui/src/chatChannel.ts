// What the parts of the chat are given of the channel they show (issue 595): who is in it and what
// the user may do there - provided once by the chat, and read by a conversation's list and the
// field the user writes in, rather than handed down to each of them, as the messages read what
// they need of it (`shownChannel`).

import {createContext, useContext} from 'react';
import type {ChannelAttachments} from './api';
import type {Answer} from './conversation';
import type {
  ActorId,
  ChannelParticipant,
  ChannelThread,
  ChannelResponse,
  CommandDescription,
  Compartment,
  Message,
  ToolCall,
} from './types';

/**
 * What the parts of the chat are given of the channel: who is in it, and what is done there in
 * the user's name. Each action reports what fails itself.
 */
export interface ChatChannel {
  /** The user's actor, whose messages are their own (issue 407). */
  me: ActorId;
  /** The channel as it stands; `null` until it is known. */
  channel: ChannelResponse | null;
  /** Its participants, sorted by name. */
  participants: ChannelParticipant[];
  /** Whether the user takes part in it: only a participant changes what is said there. */
  takesPart: boolean;
  /** The compartments a message may be released to (issue 133). */
  compartments: Compartment[];
  /** The commands the user may run here, for `/` completion (issue 28). */
  commands: CommandDescription[];
  /**
   * The attachments of the channel's messages (issue 282), where what is attached to a message
   * written or changed here is uploaded - always given to the chat, where `ShownChannel`, which
   * also serves a record that writes nothing, may lack them.
   */
  attachments: ChannelAttachments;
  /** Its threads that are open (issue 595), each marked at the message it hangs at. */
  threads: ChannelThread[];
  /** Shows the thread `thread` in its pane beside the line (issue 595). */
  openThread: (thread: string) => void;
  /** Asks the agent that said `message` to answer again instead (issue 234). */
  retry: (message: Message) => Promise<void>;
  /** Asks for another answer instead of `answer`, still under way (issue 235). */
  retryAnswer: (answer: Answer) => Promise<void>;
  /** Asks for an answer anew after the turn given up that `message` tells of (issue 259). */
  retryGivenUp: (message: Message) => Promise<void>;
  /** Changes the user's own `message` (issue 234); whether it was changed. */
  modify: (message: Message, content: string, attachments: string[]) => Promise<boolean>;
  /** Takes the user's own `message` back (issue 234). */
  revoke: (message: Message) => Promise<void>;
  /** Asks how `message` is to be classified anew (issue 239). */
  reclassify: (message: Message) => void;
  /** Shows what the agent saw when it said `message` (issue 41). */
  showContext: (message: Message) => void;
  /** Interrupts the call `call` of `message` alone (issue 276). */
  interruptCall: (message: Message, call: ToolCall) => Promise<void>;
  /** Stops the answer of the agent `sender`, an `actorKey`, alone (issue 235). */
  stopAnswer: (sender: string) => Promise<void>;
}

/** The channel the parts of the chat below show; provided by the chat alone. */
export const ChatChannelContext = createContext<ChatChannel | null>(null);

/**
 * The channel the part of the chat being shown is of.
 *
 * @throws outside the chat, which provides it.
 */
export function useChatChannel(): ChatChannel {
  const actions = useContext(ChatChannelContext);
  if (!actions) throw new Error('A part of the chat is shown outside the chat');
  return actions;
}
