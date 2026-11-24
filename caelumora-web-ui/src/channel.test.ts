// The tests of what the channel's stream changes of the channel as the chat holds it (issue 595).

import {describe, expect, it} from 'vitest';
import type {ChannelResponse, ChannelThread} from './types';
import {withChannelEvent, withThreadOpened, withThreadRead} from './channel';
import {AGENT, CHANNEL, ME, THREAD, channelOf, envelope, message, participant} from './testing';
import {describeThread} from './threads';

/** The channel with `AGENT` and `ME` taking part, and `threads` open. */
function channel(threads: ChannelThread[] = []): ChannelResponse {
  return {...channelOf([participant(AGENT, 'agent'), participant(ME, 'me')]), threads};
}

const opened: ChannelThread = {id: THREAD, opening: {origin: {thread: CHANNEL, message: 'm1'}, by: AGENT, circle: 'closed'}, recipient: AGENT};

describe('withChannelEvent', () => {
  it('tells where an agent answers: the thread a step of its turn is announced in', () => {
    const working = withChannelEvent(channel(), envelope({StateChanged: 'Planning'}, AGENT, THREAD), ME);
    expect(working.participants[0]).toMatchObject({state: 'Planning', answering_in: THREAD});
    const waiting = withChannelEvent(working, envelope({StateChanged: 'WaitingForUser'}), ME);
    expect(waiting.participants[0]).toMatchObject({state: 'WaitingForUser', answering_in: undefined});
  });

  it('lets a thread closed go, and leaves the channel as it is otherwise', () => {
    const held = channel([opened]);
    expect(withChannelEvent(held, envelope({ThreadClosed: {thread: THREAD}}), ME).threads).toEqual([]);
    expect(withChannelEvent(held, envelope({ThreadClosed: {thread: 'elsewhere'}}), ME)).toBe(held);
    expect(withChannelEvent(held, envelope({ResponseStarted: {turn_id: 'turn-1'}}), ME)).toBe(held);
  });

  it('counts what others say in a thread as new there, and marking it read clears it', () => {
    const held = channel([opened]);
    const said = (sender: typeof ME, thread: string) =>
      envelope({CompleteMessageGenerated: {...message(`m-${thread}`, sender), addressees: [ME]}}, sender, thread);
    const counted = withChannelEvent(held, said(AGENT, THREAD), ME);
    expect(counted.threads?.[0].unread).toEqual({messages: 1, addressed: true, read_up_to: undefined});
    expect(withChannelEvent(held, said(ME, THREAD), ME)).toBe(held);
    expect(withChannelEvent(held, said(AGENT, CHANNEL), ME)).toBe(held);
    const read = withThreadRead(counted, THREAD, 'm-thread');
    expect(read.threads?.[0].unread).toEqual({messages: 0, addressed: false, read_up_to: 'm-thread'});
  });
});

describe('withThreadOpened', () => {
  it('lists a thread just opened once', () => {
    const listed = withThreadOpened(channel(), opened);
    expect(listed.threads).toEqual([opened]);
    expect(withThreadOpened(listed, opened)).toBe(listed);
  });
});

describe('describeThread', () => {
  it('tells whose sub-agent a thread is of, who takes part in it, and whether one answers there', () => {
    const answering = {...participant(AGENT, 'researcher'), threads: [THREAD], answering_in: THREAD};
    expect(describeThread(opened, [answering, participant(ME, 'me')])).toEqual({
      title: 'Thread of researcher',
      takingPart: [answering],
      answering: true,
    });
    expect(describeThread({...opened, recipient: undefined}, []).title).toBe('Thread');
  });
});
