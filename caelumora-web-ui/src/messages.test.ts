// The tests of how the chat reads what the channel says (issue 595): which events the line's view
// shows, and how tool calls and their results are shown together.

import {describe, expect, it} from 'vitest';
import type {Message, ToolCall, ToolPresentation, ToolResult, ToolResultStatus} from './types';
import {isSaidInLine, isShownInLine, isShownInThread, mergeToolResults, withoutQuietCalls} from './messages';
import {AGENT, CHANNEL, ME, THREAD, envelope, message} from './testing';

describe('isShownInLine', () => {
  it('shows what happens in the line', () => {
    expect(isShownInLine(envelope({CompleteMessageGenerated: message('m1')}), ME)).toBe(true);
    expect(isShownInLine(envelope({ResponseStarted: {turn_id: 'turn-1'}}), ME)).toBe(true);
  });

  it('leaves out what is said in another thread, but a message that mentions the reader', () => {
    const said = message('m1');
    expect(isShownInLine(envelope({CompleteMessageGenerated: said}, AGENT, THREAD), ME)).toBe(false);
    expect(isShownInLine(envelope({TurnFinished: {turn_id: 'turn-1', messages: []}}, AGENT, THREAD), ME)).toBe(false);
    const mentioning = {...said, addressees: [ME]};
    expect(isShownInLine(envelope({CompleteMessageGenerated: mentioning}, AGENT, THREAD), ME)).toBe(true);
  });

  it('shows of another thread what waits for a person, and what happens in the whole channel', () => {
    const asked = {QuestionClosed: {id: 'question-1'}};
    expect(isShownInLine(envelope(asked, AGENT, THREAD), ME)).toBe(true);
    const opened = {ThreadOpened: {thread: THREAD, origin: {thread: CHANNEL, message: 'm1'}}};
    expect(isShownInLine(envelope(opened, AGENT, THREAD), ME)).toBe(true);
  });
});

describe('isShownInThread', () => {
  it('shows what happens in the thread and in the whole channel, nothing of the line', () => {
    expect(isShownInThread(envelope({CompleteMessageGenerated: message('m1')}, AGENT, THREAD), THREAD)).toBe(true);
    expect(isShownInThread(envelope({CompleteMessageGenerated: message('m1')}), THREAD)).toBe(false);
    const revoked = {MessageRevoked: {message: 'm1', at: '2026-11-23T12:01:00Z'}};
    expect(isShownInThread(envelope(revoked), THREAD)).toBe(true);
  });
});

describe('isSaidInLine', () => {
  it('shows a message said in the line, or one of another thread that mentions the reader', () => {
    const said = {sessionId: 'session-1', channelId: CHANNEL, thread: CHANNEL, message: message('m1')};
    expect(isSaidInLine(said, ME)).toBe(true);
    expect(isSaidInLine({...said, thread: THREAD}, ME)).toBe(false);
    expect(isSaidInLine({...said, thread: THREAD, message: {...said.message, addressees: [ME]}}, ME)).toBe(true);
  });
});

describe('tool calls', () => {
  const call = (id: string, presentation?: ToolPresentation): ToolCall =>
    ({id, function_name: 'run', arguments: {}, presentation});
  const result = (id: string, status: ToolResultStatus = 'ok', presentation?: ToolPresentation): ToolResult =>
    ({id, tool_name: 'run', content: 'done', status, presentation, classification: {level: 'public'}});

  it('shows a result with the call it answers, and leaves the tool message out', () => {
    const calling: Message = {...message('m1'), tool_calls: [call('c1')]};
    const answered: Message = {id: 'm2', role: 'Tool', content: null, tool_results: [result('c1')]};
    const merged = mergeToolResults([calling, answered]);
    expect(merged.map(m => m.id)).toEqual(['m1']);
    expect(merged[0].tool_results?.map(r => [r.id, r.message_id])).toEqual([['c1', 'm2']]);
  });

  it('leaves a result whose call is not shown where it is', () => {
    const answered: Message = {id: 'm2', role: 'Tool', content: null, tool_results: [result('c1')]};
    expect(mergeToolResults([message('m1'), answered])).toEqual([message('m1'), answered]);
  });

  it('leaves out a quiet call that did not fail, and a message that held nothing else', () => {
    const quiet: Message = {...message('m1', AGENT, ''), tool_calls: [call('c1', 'quiet')], tool_results: [result('c1', 'ok', 'quiet')]};
    expect(withoutQuietCalls([quiet])).toEqual([]);
    const failed: Message = {...quiet, tool_results: [result('c1', 'failed', 'quiet')]};
    expect(withoutQuietCalls([failed])).toEqual([failed]);
    const saying: Message = {...quiet, content: 'looked it up'};
    expect(withoutQuietCalls([saying])).toEqual([{...saying, tool_calls: [], tool_results: []}]);
  });
});
