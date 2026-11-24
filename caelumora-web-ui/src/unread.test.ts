// The tests of what the session list tells of what waits for the user in a channel (issue 594):
// what waits, and whether more waits than before, which it notifies of.

import {describe, expect, it} from 'vitest';
import {waitingLabel, waitsMore} from './unread';

describe('waitsMore', () => {
  it('tells an approval or questions asked since', () => {
    expect(waitsMore(undefined, {approvals: 1, questions: 0})).toBe(true);
    expect(waitsMore({approvals: 1, questions: 0}, {approvals: 1, questions: 1})).toBe(true);
  });

  it('is not what ended, nor what stays', () => {
    expect(waitsMore({approvals: 1, questions: 1}, {approvals: 0, questions: 1})).toBe(false);
    expect(waitsMore({approvals: 1, questions: 0}, {approvals: 1, questions: 0})).toBe(false);
  });
});

describe('waitingLabel', () => {
  it('counts what waits, and is empty where nothing does', () => {
    expect(waitingLabel({approvals: 1, questions: 2})).toBe('1 approval, 2 sets of questions');
    expect(waitingLabel({approvals: 0, questions: 1})).toBe('1 set of questions');
    expect(waitingLabel({approvals: 0, questions: 0})).toBe('');
    expect(waitingLabel(undefined)).toBe('');
  });
});
