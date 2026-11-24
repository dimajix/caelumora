// The tests of what the views read of an event of the global stream: here for whom what waits in a
// channel changed (issue 594).

import {describe, expect, it} from 'vitest';
import type {GlobalEvent} from './types';
import {waitingChanged} from './globalEvents';
import {CHANNEL, ME} from './testing';

const SESSION = 'session-1';

/** A channel's custom change `name` with `data`, as the global stream sends it. */
function custom(name: string, data: unknown): GlobalEvent {
  return {kind: 'channel', id: CHANNEL, within: SESSION, change: {Custom: {name, data}}};
}

describe('waitingChanged', () => {
  it('tells where and for whom what waits changed', () => {
    expect(waitingChanged(custom('waiting_changed', ME))).toEqual({sessionId: SESSION, channelId: CHANNEL, user: ME});
  });

  it('is nothing for another change of the channel', () => {
    expect(waitingChanged(custom('said', {message: {}, thread: CHANNEL}))).toBeNull();
    expect(waitingChanged({kind: 'channel', id: CHANNEL, within: SESSION, change: 'Deleted'})).toBeNull();
  });
});
