// The tests of a conversation of a channel (issue 595): how the stream's events and the pages of
// the history change what is shown of it.

import {describe, expect, it} from 'vitest';
import type {HistoryPage, Message} from './types';
import {
  NO_CONVERSATION,
  answersUnderWay,
  conversationOf,
  withAdded,
  withAnswersUnderWay,
  withEarlierPage,
  withEvent,
  withLastPage,
  withReplaced,
  type Conversation,
} from './conversation';
import {activityUnderWay} from './messages';
import {AGENT, CHANNEL, ME, OTHER_AGENT, THREAD, envelope, message, participant} from './testing';

/** `conversation` after each of `events`, in their order. */
function after(conversation: Conversation, ...events: Parameters<typeof withEvent>[1][]): Conversation {
  return events.reduce(withEvent, conversation);
}

/** A page of the history of `messages` said, and `earlier` before it. */
function page(messages: Message[], earlier: number | null = null): HistoryPage<Message> {
  return {items: messages.map((said, seq) => ({seq, said})), earlier};
}

const nameOf = () => 'agent';

describe('withEvent', () => {
  it('shows an answer under way as it streams, and keeps its turn once its message is complete', () => {
    const streaming = after(
      NO_CONVERSATION,
      envelope({ResponseStarted: {turn_id: 'turn-1'}}),
      envelope({PartialMessageGenerated: {role: 'Assistant', content: 'hel'}}),
      envelope({PartialMessageGenerated: {role: 'Assistant', content: 'lo'}}),
    );
    expect(streaming.answers).toMatchObject([{turnId: 'turn-1', content: 'hello', internal: false}]);

    const said = withEvent(streaming, envelope({CompleteMessageGenerated: message('m1')}));
    expect(said.messages.map(m => m.id)).toEqual(['m1']);
    expect(said.answers).toMatchObject([{turnId: 'turn-1', content: '', reasoning: ''}]);
  });

  it('keeps the answers of several agents apart, in the order they began', () => {
    const both = after(
      NO_CONVERSATION,
      envelope({ResponseStarted: {turn_id: 'turn-a'}}),
      envelope({ResponseStarted: {turn_id: 'turn-b'}}, OTHER_AGENT),
      envelope({PartialMessageGenerated: {role: 'Assistant', content: 'one'}}),
      envelope({IntermediatePartialGenerated: {role: 'Assistant', content: 'two'}}, OTHER_AGENT),
    );
    expect(both.answers).toMatchObject([
      {turnId: 'turn-a', content: 'one', internal: false},
      {turnId: 'turn-b', content: 'two', internal: true},
    ]);

    const oneEnded = withEvent(both, envelope({TurnFinished: {turn_id: 'turn-a', messages: []}}));
    expect(oneEnded.answers).toMatchObject([{turnId: 'turn-b', content: 'two'}]);

    // An agent that begins anew answers after those under way.
    const again = after(both, envelope({ResponseStarted: {turn_id: 'turn-c'}}));
    expect(again.answers.map(a => a.turnId)).toEqual(['turn-b', 'turn-c']);
  });

  it('shows a message that arrives twice once', () => {
    const said = envelope({CompleteMessageGenerated: message('m1')});
    expect(after(NO_CONVERSATION, said, said).messages).toHaveLength(1);
  });

  it('ends the answer of a turn that finished, and what it did of its own accord', () => {
    const working = after(
      NO_CONVERSATION,
      envelope({ResponseStarted: {turn_id: 'turn-1'}}),
      envelope({ActivityStarted: {turn_id: 'turn-1', activity: 'compacting'}}),
    );
    expect(working.messages.map(m => m.id)).toEqual([activityUnderWay('turn-1')]);

    const finished = withEvent(working, envelope({TurnFinished: {turn_id: 'turn-1', messages: []}}));
    expect(finished.answers).toEqual([]);
    expect(finished.messages).toEqual([]);
  });

  it('tells of a turn given up once, and ends its answer', () => {
    const givenUp = envelope({TurnGivenUp: {turn_id: 'turn-1', reason: {kind: 'failed', message: 'down'}}});
    const ended = after(NO_CONVERSATION, envelope({ResponseStarted: {turn_id: 'turn-1'}}), givenUp, givenUp);
    expect(ended.answers).toEqual([]);
    expect(ended.messages.filter(m => m.givenUp)).toHaveLength(1);
  });

  it('marks the calls of a participant that left as abandoned, and ends its answer', () => {
    const calling: Message = {...message('m1'), tool_calls: [{id: 'call-1', function_name: 'run', arguments: {}}]};
    const left = after(
      NO_CONVERSATION,
      envelope({ResponseStarted: {turn_id: 'turn-1'}}),
      envelope({CompleteMessageGenerated: calling}),
      envelope({ParticipantLeft: {participant: AGENT, name: 'agent', until: 3}}, ME),
    );
    expect(left.answers).toEqual([]);
    expect(left.messages[0].abandoned).toBe(true);
    expect(left.messages[1].participant).toBe('left');
  });

  it('takes a message back in its place', () => {
    const shown = conversationOf(page([message('m1'), message('m2')]), nameOf);
    const revoked = withEvent(shown, envelope({MessageRevoked: {message: 'm1', at: '2026-11-23T12:01:00Z'}}));
    expect(revoked.messages.map(m => [m.id, m.content, m.revoked])).toEqual([
      ['m1', null, '2026-11-23T12:01:00Z'],
      ['m2', 'hello', undefined],
    ]);
  });

  it('shows a notice once, read from the history and then announced live', () => {
    const cutOff = {message: 'm1', max_agent_hops: 3, not_asked: [OTHER_AGENT]};
    const read = conversationOf({items: [{seq: 5, chain_cut_off: {...cutOff, at: '2026-11-23T12:00:00Z'}}], earlier: null}, nameOf);
    const live = envelope({ChainCutOff: cutOff}, 'System');
    expect(withEvent(read, {...live, context: {...live.context, position: 5}})).toBe(read);

    const givenUp = {turn_id: 'turn-1', reason: {kind: 'retried' as const}};
    const readGivenUp = conversationOf({items: [{seq: 6, turn_given_up: {...givenUp, actor: AGENT, at: '2026-11-23T12:00:00Z'}}], earlier: null}, nameOf);
    expect(withEvent(readGivenUp, envelope({TurnGivenUp: givenUp})).messages).toHaveLength(1);
  });

  it('leaves the conversation as it is where an event changes nothing of it', () => {
    // A notice the history records is known by its position there.
    const asked = envelope({ResponseRequested: {reason: 'nudged'}}, ME);
    const nudge = {...asked, context: {...asked.context, position: 5}};
    const nudged = withEvent(NO_CONVERSATION, nudge);
    expect(withEvent(nudged, nudge)).toBe(nudged);
    expect(withEvent(nudged, envelope({StateChanged: 'Idle'}))).toBe(nudged);
    // What changes a message not shown, an answer not under way, or adds what is shown already.
    expect(withEvent(nudged, envelope({MessageRevoked: {message: 'elsewhere', at: '2026-11-23T12:01:00Z'}}))).toBe(nudged);
    expect(withEvent(nudged, envelope({TurnFinished: {turn_id: 'turn-1', messages: []}}))).toBe(nudged);
    const given = withEvent(nudged, envelope({GivenToRead: {recipient: ME, message: message('m1')}}, {Delegate: 'thread-1'}));
    expect(withEvent(given, envelope({GivenToRead: {recipient: ME, message: message('m1')}}, {Delegate: 'thread-1'}))).toBe(given);
    const said = withEvent(nudged, envelope({CompleteMessageGenerated: message('m2')}));
    expect(withEvent(said, envelope({CompleteMessageGenerated: message('m2')}))).toBe(said);
  });
});

describe('pages of the history', () => {
  it('shows the last page in place of what was shown, keeping what came after it', () => {
    const before = {...message('m0'), timestamp: '2026-11-23T11:00:00Z'};
    const later = {...message('m3'), timestamp: '2026-11-23T12:05:00Z'};
    const told: Message = {role: 'System', content: 'only for you', timestamp: '2026-11-23T12:06:00Z'};
    const answering = after(NO_CONVERSATION, envelope({ResponseStarted: {turn_id: 'turn-1'}}));
    const shown: Conversation = {...answering, messages: [before, later, told]};
    const last = page([{...message('m1'), timestamp: '2026-11-23T12:00:00Z'}], 7);
    const reloaded = withLastPage(shown, last, nameOf);
    expect(reloaded.messages.map(m => m.id)).toEqual(['m1', 'm3']);
    expect(reloaded.earlier).toBe(7);
    expect(reloaded.answers).toBe(answering.answers);
  });

  it('puts an earlier page in front of what is shown, each message once', () => {
    const shown = conversationOf(page([message('m2'), message('m3')], 4), nameOf);
    const earlier = withEarlierPage(shown, page([message('m1'), message('m2')]), nameOf);
    expect(earlier.messages.map(m => m.id)).toEqual(['m1', 'm2', 'm3']);
    expect(earlier.earlier).toBeNull();
  });
});

describe('a message from elsewhere', () => {
  it('replaces the one shown of its id, and adds nothing', () => {
    const shown = conversationOf(page([message('m1')]), nameOf);
    const standing = {...message('m1'), content: 'as it stands'};
    expect(withReplaced(shown, standing).messages.map(m => m.content)).toEqual(['as it stands']);
    expect(withReplaced(shown, message('m2'))).toBe(shown);
  });

  it('is added once, or always where it has no id', () => {
    const shown = conversationOf(page([message('m1')]), nameOf);
    expect(withAdded(shown, {...message('m1'), content: 'again'})).toBe(shown);
    const told: Message = {role: 'System', content: 'only for you'};
    expect(withAdded(withAdded(shown, told), told).messages).toHaveLength(3);
  });
});

describe('withAnswersUnderWay', () => {
  it('adds the answers a fetch found, but where their senders answer already', () => {
    const answering = after(NO_CONVERSATION, envelope({ResponseStarted: {turn_id: 'turn-new'}}));
    const found = [
      {sender: 'Agent:agent-1', name: 'agent', turnId: 'turn-old', reasoning: '', content: '', internal: false},
      {sender: 'Agent:agent-2', name: 'other', turnId: 'turn-2', reasoning: '', content: '', internal: false},
    ];
    expect(withAnswersUnderWay(answering, found).answers.map(a => a.turnId)).toEqual(['turn-new', 'turn-2']);
    expect(withAnswersUnderWay(answering, found.slice(0, 1))).toBe(answering);
  });
});

describe('answersUnderWay', () => {
  it('begins the answers of the participants a snapshot found in a turn of the thread', () => {
    const answering = {...participant(AGENT, 'agent'), turns_under_way: {[CHANNEL]: 'turn-1'}};
    const elsewhere = {...participant(OTHER_AGENT, 'other'), turns_under_way: {[THREAD]: 'turn-2'}};
    expect(answersUnderWay([answering, elsewhere, participant(ME, 'me')], CHANNEL)).toEqual([
      {sender: 'Agent:agent-1', name: 'agent', turnId: 'turn-1', reasoning: '', content: '', internal: false},
    ]);
    expect(answersUnderWay([answering, elsewhere], THREAD).map(a => a.turnId)).toEqual(['turn-2']);
  });
});
