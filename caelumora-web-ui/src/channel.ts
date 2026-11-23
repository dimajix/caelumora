// The channel as the chat holds it between the times it fetches it (issue 595): what its stream
// changes - where its agents stand and answer, which of its threads are open, what is new in each -
// and what the user's reading changes of it.

import type {ActorId, ActorState, ChannelResponse, ChannelThread, Envelope} from './types';
import {actorKey} from './actor';
import {isHeard} from './messages';
import {countedIn, isOthers} from './unread';

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
 * `channel` with `thread` read up to its message `message` (issue 595): nothing new in it.
 */
export function withThreadRead(channel: ChannelResponse, thread: string, message: string): ChannelResponse {
  const unread = {messages: 0, mentioned: false, read_up_to: message};
  return {...channel, threads: channel.threads?.map(t => t.id === thread ? {...t, unread} : t)};
}

/**
 * `channel` as `envelope` changes it: where its sender stands (issue 243) and the thread it answers
 * in - a step of a turn is announced in the turn's thread (issue 595) -, a message heard of
 * someone other than `me` counted as new in the thread it is said in, and a thread closed no longer
 * among its threads. The same `channel` where the envelope changes nothing of it.
 */
export function withChannelEvent(channel: ChannelResponse, envelope: Envelope, me: ActorId): ChannelResponse {
  const event = envelope.event;
  if (typeof event === 'string') return channel;
  const said = event.CompleteMessageGenerated;
  if (said) {
    const thread = envelope.context.thread_id;
    const theirs = isOthers(said, me) && isHeard(said);
    if (!theirs || !channel.threads?.some(t => t.id === thread)) return channel;
    const threads = channel.threads.map(t => t.id === thread ? {...t, unread: countedIn(t.unread, said, me)} : t);
    return {...channel, threads};
  }
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
