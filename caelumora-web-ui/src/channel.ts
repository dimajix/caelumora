// What the channel's stream changes of the channel as the chat holds it (issue 595): where its
// agents stand and answer, and which of its threads are open - what the chat keeps up between the
// times it fetches the channel.

import type {ActorState, ChannelResponse, ChannelThread, Envelope} from './types';
import {actorKey} from './actor';

/**
 * The states in which an agent answers, as the kernel's `ActorState::answers` says: a step of a
 * turn runs - not one that waits for a person, which is set aside (issue 602).
 */
const ANSWERING: ActorState[] = ['Planning', 'ExecutingTools', 'Reflecting', 'Cancelling', 'Busy'];

/**
 * `channel` with `thread`, just opened (issue 595), among its threads - the same `channel` where it
 * lists it already.
 */
export function withThreadOpened(channel: ChannelResponse, thread: ChannelThread): ChannelResponse {
  if (channel.threads?.some(listed => listed.id === thread.id)) return channel;
  return {...channel, threads: [...(channel.threads ?? []), thread]};
}

/**
 * `channel` as `envelope` changes it: where its sender stands (issue 243) and the thread it answers
 * in - a step of a turn is announced in the turn's thread (issue 595) -, and a thread closed no
 * longer among its threads. The same `channel` where the envelope changes nothing of it.
 */
export function withChannelEvent(channel: ChannelResponse, envelope: Envelope): ChannelResponse {
  const event = envelope.event;
  if (typeof event === 'string') return channel;
  if (event.StateChanged) {
    const state = event.StateChanged;
    const sender = actorKey(envelope.context.sender_id);
    const answering_in = ANSWERING.includes(state) ? envelope.context.thread_id : undefined;
    const participants = channel.participants.map(m => actorKey(m.id) === sender ? {...m, state, answering_in} : m);
    return {...channel, participants};
  }
  if (event.ThreadClosed) {
    const closed = event.ThreadClosed.thread;
    if (!channel.threads?.some(thread => thread.id === closed)) return channel;
    return {...channel, threads: channel.threads.filter(thread => thread.id !== closed)};
  }
  return channel;
}
