// What the web UI's tests build their cases from (issue 595): a channel, the actors in it, and the
// envelopes of its stream and the messages they carry - each test writes beside them only what it
// differs in. Imported by tests alone.

import type {ActorEvent, ActorId, ChannelParticipant, ChannelResponse, Envelope, Message} from './types';
import {agentIdOf} from './actor';

/** The channel the cases happen in: its id is its line's (issue 557). */
export const CHANNEL = 'channel-1';

/** Another thread of it. */
export const THREAD = 'thread-1';

/** The user who reads, and the agents that answer. */
export const ME: ActorId = {User: 'user-1'};
export const AGENT: ActorId = {Agent: 'agent-1'};
export const OTHER_AGENT: ActorId = {Agent: 'agent-2'};

/** `id` as a participant of the channel, named `name`, who may do nothing there. */
export function participant(id: ActorId, name: string): ChannelParticipant {
  return {
    id,
    name,
    handle: name,
    kind: agentIdOf(id) !== null ? 'agent' : 'user',
    can_read_context: false,
    can_read_tools: false,
    can_read_components: false,
    can_approve: false,
    can_retry: false,
    can_cancel: false,
    can_delete: false,
    can_leave: false,
  };
}

/** A message said by `sender`, in words, with the id `id`. */
export function message(id: string, sender: ActorId = AGENT, content = 'hello'): Message {
  return {id, role: agentIdOf(sender) !== null ? 'Assistant' : 'User', sender_id: sender, content};
}

/** `event`, sent by `sender` in `thread` of the channel - its line unless named. */
export function envelope(event: ActorEvent, sender: ActorId = AGENT, thread = CHANNEL): Envelope {
  return {
    timestamp: '2026-11-23T12:00:00Z',
    event,
    context: {
      session_id: 'session-1',
      channel_id: CHANNEL,
      thread_id: thread,
      sender_id: sender,
      sender_name: agentIdOf(sender) !== null ? 'agent' : 'someone',
      event_id: 'event-1',
      correlation_id: 'correlation-1',
      span_id: 'span-1',
      span_depth: 0,
    },
  };
}

/** The channel the cases happen in, with `participants`, as its stream's snapshot gives it. */
export function channelOf(participants: ChannelParticipant[]): ChannelResponse {
  return {
    id: CHANNEL,
    name: 'main',
    participants,
    moderator: null,
    responder_policy: null,
    classification: {level: 'internal'},
    language: null,
    effective_language: null,
    approval_rules: [],
    uses_workspace: false,
    can_change: false,
    former_participants: [],
  };
}
