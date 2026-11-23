// One conversation of a channel as the chat shows it (issue 595) - the line, or a thread: what was
// said in it and noticed, the answers under way in it, and where its earlier pages are read; and
// how what the channel's stream says, and the pages of its history, change it. What changes the
// channel as a whole - its participants, what calls used, approvals and questions - is the chat's.

import type {ActorId, ChannelParticipant, Envelope, HistoryPage, Message} from './types';
import {actorKey, sameActor} from './actor';
import {
  activityNotice,
  activityUnderWay,
  chainCutOffNotice,
  channelCommandNotice,
  chatMessages,
  commandFailedNotice,
  commandNotice,
  eventRefusedNotice,
  eventRejectedNotice,
  givenBack,
  givenUpNotice,
  mergeToolResults,
  notDrawnInNotice,
  nudgeNotice,
  participantNotice,
  planStepMark,
  responderUndecidedNotice,
  retryRefusedNotice,
  threadNotice,
  withoutQuietCalls,
} from './messages';

/**
 * An agent's answer under way (issue 203): what of it has streamed in since its last complete
 * message, and the plan step it is at. Several agents can answer at once, each with an answer of
 * its own.
 */
export interface Answer {
  /** Its sender, as `actorKey`. */
  sender: string;
  name: string | null;
  /**
   * The turn it is (issue 235): from `ResponseStarted`, or - for an answer that began before the
   * page was loaded - from its first message.
   */
  turnId: string | null;
  reasoning: string;
  content: string;
  /** Whether what streams in is a step of a plan (issue 263), shown in the step's block. */
  internal: boolean;
}

/** One conversation of a channel as shown - the line, or a thread. */
export interface Conversation {
  /** What was said and noticed, as it came; a call and its result are shown together as drawn. */
  messages: Message[];
  /**
   * The answers under way, in the order they began (issue 203). Shown below every complete
   * message: a message joins the history when it is complete, so an answer that ends first is
   * shown first, as a reload shows it.
   */
  answers: Answer[];
  /** What the page before those shown is read before (issue 218); `null` once there is none. */
  earlier: number | null;
}

/** A conversation nothing is known of yet. */
export const NO_CONVERSATION: Conversation = {messages: [], answers: [], earlier: null};

/** Whose name an actor has, to name whose turn ended in a page of the history (issue 258). */
type NameOf = (actor: ActorId) => string | undefined;

/** The answer of `sender`, called `name`, as it begins: nothing of it streamed yet. */
function begun(sender: string, name: string | null, turnId: string | null): Answer {
  return {sender, name, turnId, reasoning: '', content: '', internal: false};
}

/** `answers` with the answer of `sender` changed by `change` - added at the end if it has none. */
function withAnswer(answers: Answer[], sender: string, name: string | null, change: (answer: Answer) => Answer): Answer[] {
  const index = answers.findIndex(a => a.sender === sender);
  if (index < 0) return [...answers, change(begun(sender, name, null))];
  return answers.map((a, i) => i === index ? change({...a, name: name ?? a.name}) : a);
}

/**
 * The answers under way that `participants`, as a snapshot found them, are giving in the line
 * (issue 240): what streamed before is not kept, what follows streams in.
 */
export function answersUnderWay(participants: ChannelParticipant[]): Answer[] {
  return participants.flatMap(m => m.turn_under_way ? [begun(actorKey(m.id), m.name, m.turn_under_way)] : []);
}

/** `messages` as the chat shows them: each result with its call, the quiet calls left out. */
export function shownOf(messages: Message[]): Message[] {
  return withoutQuietCalls(mergeToolResults(messages));
}

/**
 * A conversation as a snapshot of the channel's stream, or the last page of its history, starts
 * it, with the answers under way the snapshot found.
 */
export function conversationOf(page: HistoryPage<Message>, nameOf: NameOf, answers: Answer[] = []): Conversation {
  return {messages: chatMessages(page.items, nameOf), answers, earlier: page.earlier};
}

/**
 * `conversation` with `page`, the last page of its history, in place of what was shown, as a
 * snapshot would show it (issue 218): the pages before it that were loaded are let go, and what
 * arrived after it was read is kept.
 */
export function withLastPage(conversation: Conversation, page: HistoryPage<Message>, nameOf: NameOf): Conversation {
  const data = chatMessages(page.items, nameOf);
  const known = new Set(data.flatMap(m => m.id ? [m.id] : []));
  const read = data[data.length - 1]?.timestamp;
  const after = conversation.messages.filter(m => m.id && !known.has(m.id) && read && m.timestamp && m.timestamp > read);
  return {...conversation, messages: [...data, ...after], earlier: page.earlier};
}

/** `conversation` with `page`, the page of its history before those shown, in front of them. */
export function withEarlierPage(conversation: Conversation, page: HistoryPage<Message>, nameOf: NameOf): Conversation {
  const data = chatMessages(page.items, nameOf);
  const known = new Set(data.map(m => m.id));
  const kept = conversation.messages.filter(m => !m.id || !known.has(m.id));
  return {...conversation, messages: [...data, ...kept], earlier: page.earlier};
}

/**
 * `conversation` with `message` in place of the one of its id - one read back as it stands -; as
 * it is where it shows none of that id.
 */
export function withReplaced(conversation: Conversation, message: Message): Conversation {
  const {messages} = conversation;
  if (!message.id || !messages.some(m => m.id === message.id)) return conversation;
  return {...conversation, messages: messages.map(m => m.id === message.id ? message : m)};
}

/**
 * `conversation` with `message` after those shown, unless one of its id is there: one said that a
 * stream other than the channel's told, or what the user is told alone, without an id.
 */
export function withAdded(conversation: Conversation, message: Message): Conversation {
  const {messages} = conversation;
  if (message.id && messages.some(m => m.id === message.id)) return conversation;
  return {...conversation, messages: [...messages, message]};
}

/**
 * `messages` with `notice` after them, unless one of its id is there: shown once, live and from
 * the history (issue 417).
 */
function once(messages: Message[], notice: Message): Message[] {
  return notice.id && messages.some(m => m.id === notice.id) ? messages : [...messages, notice];
}

/** `messages` with `notice` in place of the one of its id, or after them. */
function replacing(messages: Message[], notice: Message): Message[] {
  return messages.some(m => m.id === notice.id) ? messages.map(m => m.id === notice.id ? notice : m) : [...messages, notice];
}

/**
 * `conversation` as `envelope`, of the channel's stream, changes it: what it says and notices,
 * and the answers under way. An envelope that says nothing of a conversation leaves it as it is.
 */
export function withEvent(conversation: Conversation, envelope: Envelope): Conversation {
  const {messages, answers} = conversation;
  const event = envelope.event;
  if (typeof event === 'string') return conversation;
  const sender = envelope.context.sender_id;
  const answerer = actorKey(sender);
  const answererName = envelope.context.sender_name || null;
  const at = envelope.timestamp;
  const position = envelope.context.position;
  // What changed of it - the same conversation where nothing did, which is drawn again by nothing.
  const chat = (change: (messages: Message[]) => Message[]): Conversation => {
    const changed = change(messages);
    return changed === messages ? conversation : {...conversation, messages: changed};
  };
  const under = (change: (answers: Answer[]) => Answer[]): Conversation => {
    const changed = change(answers);
    return changed === answers ? conversation : {...conversation, answers: changed};
  };
  // The answer of the envelope's sender, if it has one, has ended.
  const ended = (answers: Answer[]) => answers.filter(a => a.sender !== answerer);
  if (event.ResponseRequested) {
    // Someone asks you to respond (issue 278) - an agent is asked so, too, but a user only by a
    // nudge.
    return chat(prev => once(prev, nudgeNotice(answererName ?? undefined, at, position)));
  }
  if (event.ResponseStarted) {
    // A new answer begins after those under way, as the turn it names.
    const turnId = event.ResponseStarted.turn_id;
    return under(prev => withAnswer(ended(prev), answerer, answererName, a => ({...a, turnId})));
  }
  if (event.PartialMessageGenerated || event.IntermediatePartialGenerated) {
    // A step of a plan streams in as the answer does (issue 258).
    const partial = (event.PartialMessageGenerated ?? event.IntermediatePartialGenerated)!;
    const internal = !!event.IntermediatePartialGenerated;
    return under(prev => withAnswer(prev, answerer, answererName, a => ({
      ...a,
      turnId: a.turnId ?? partial.turn_id ?? null,
      internal,
      reasoning: a.reasoning + (partial.reasoning ?? ''),
      content: a.content + (partial.content ?? ''),
    })));
  }
  if (event.CompleteMessageGenerated || event.IntermediateMessageGenerated) {
    // A step of a plan is shown as any message, folded with the other steps of its turn (issue
    // 258). A message can arrive twice: in the snapshot and as an event. Kept as it came, as the
    // snapshot's are; a call and its result are shown together as the chat is drawn.
    const said = (event.CompleteMessageGenerated ?? event.IntermediateMessageGenerated)!;
    const turnId = said.turn_id ?? null;
    return {
      ...conversation,
      messages: said.id && messages.some(m => m.id === said.id) ? messages : [...messages, said],
      // Its sender may go on: what streams in next is its next message.
      answers: answers.map(a => a.sender === answerer ? {...a, turnId: a.turnId ?? turnId, reasoning: '', content: ''} : a),
    };
  }
  if (event.MessageModified) {
    // In its place, as the server reads it (`Message::modify`, issue 234): its words,
    // attachments, classification and mentions, and when.
    const modified = event.MessageModified;
    return chat(prev => prev.map(m => m.id === modified.id
      ? {...m, content: modified.content, reasoning: modified.reasoning, attachments: modified.attachments, classification: modified.classification, mentions: modified.mentions, addressees: modified.addressees, modified: modified.timestamp}
      : m));
  }
  if (event.MessageRevoked) {
    // In its place without what it said (`Message::revoke`).
    const {message, at: revoked} = event.MessageRevoked;
    return chat(prev => prev.map(m => m.id === message
      ? {...m, content: null, reasoning: undefined, attachments: [], tool_calls: [], tool_results: [], mentions: [], addressees: [], revoked}
      : m));
  }
  if (event.MessageReclassified) {
    // In its place, classified anew (`Message::reclassify`, issue 239).
    const {message, classification} = event.MessageReclassified;
    return chat(prev => prev.map(m => m.id === message ? {...m, classification} : m));
  }
  if (event.RetryAccepted) {
    // A new answer begins: shown as under way from now on. An answer under way was given up, and
    // what of it was said stays, as the history keeps it (issue 258); a completed answer is taken
    // back as `MessageRevoked` says.
    return under(prev => withAnswer(ended(prev), answerer, answererName, a => a));
  }
  if (event.RetryRefused) {
    return chat(prev => once(prev, retryRefusedNotice(event.RetryRefused!.reason, envelope.context.sender_name, at, position)));
  }
  if (event.EventRejected) {
    // What the user sent, which the channel did not let in (issue 304). A message of theirs,
    // shown as they sent it, is marked as not let in - after a reload, the history's notice
    // stands in its place. Anything else is told in a notice; a change of a message is undone by
    // what the channel holds, which the chat reads back (`withReplaced`).
    const {rejected, message, reason} = event.EventRejected;
    if (message && rejected === 'CompleteMessageGenerated') {
      return chat(prev => prev.map(m => m.id === message ? {...m, rejected: reason} : m));
    }
    return chat(prev => once(prev, eventRejectedNotice(rejected, reason, at, position)));
  }
  if (event.EventRefused) {
    // What the user sent that the actor it went to did not take (issue 567).
    const {refused, reason} = event.EventRefused;
    return chat(prev => once(prev, eventRefusedNotice(refused, reason, envelope.context.sender_name, at, position)));
  }
  if (event.TurnFinished) {
    // Its messages were shown as they were said; what it did of its own accord ended with it.
    const underWay = activityUnderWay(event.TurnFinished.turn_id);
    return {...conversation, messages: messages.filter(m => m.id !== underWay), answers: ended(answers)};
  }
  if (event.TurnGivenUp) {
    // Shown as the turn's end (issue 258), not yet part of the history: gone on reload. What it
    // did of its own accord ended with it (issue 284).
    const {turn_id, reason} = event.TurnGivenUp;
    const notice = givenUpNotice(turn_id, reason, sender, envelope.context.sender_name, at, envelope.context.span_id);
    const underWay = activityUnderWay(turn_id);
    const without = messages.filter(m => m.id !== underWay);
    return {
      ...conversation,
      messages: messages.some(m => m.id === notice.id) ? without : [...without, notice],
      answers: ended(answers),
    };
  }
  if (event.CommandExecuted) {
    const {command, args, output, to, turn_id} = event.CommandExecuted;
    if (turn_id) {
      // Carried out by an agent (issue 269): part of the history, kept once by its turn, with
      // what it answered, and to whom.
      const notice = commandNotice(turn_id, command, args, sender, envelope.context.sender_name, undefined, at, {output, private: to !== null});
      return chat(prev => replacing(prev, notice));
    }
    // A command of the channel, part of the history (issue 417).
    return chat(prev => once(prev, channelCommandNotice(command, output, envelope.context.sender_name, at, position)));
  }
  if (event.ActivityStarted) {
    // An agent does something of its own accord (issue 284): its notice shows it under way until
    // it ends.
    const {turn_id, activity} = event.ActivityStarted;
    return chat(prev => once(prev, activityNotice(turn_id, activity, sender, envelope.context.sender_name, at)));
  }
  if (event.ActivityFinished) {
    // In the place of its start, with what came of it: part of the history, kept once by when it
    // ended.
    const {turn_id, activity, output} = event.ActivityFinished;
    const notice = activityNotice(turn_id, activity, sender, envelope.context.sender_name, at, output);
    const underWay = activityUnderWay(turn_id);
    return chat(prev => prev.some(m => m.id === notice.id)
      ? prev.filter(m => m.id !== underWay)
      : prev.some(m => m.id === underWay) ? prev.map(m => m.id === underWay ? notice : m) : [...prev, notice]);
  }
  if (event.CommandStarted) {
    // An agent began a command of yours (issue 299): its notice shows it under way until the
    // answer, kept by the same turn, replaces it.
    const {command, args, turn_id} = event.CommandStarted;
    const begun = commandNotice(turn_id, command, args, sender, envelope.context.sender_name, undefined, at);
    return chat(prev => once(prev, {...begun, commandOutput: {...begun.commandOutput!, underWay: true}}));
  }
  if (event.CommandFailed) {
    // An agent did not carry out a command (issue 269): for the issuer alone, part of the
    // history (issue 417). In the place of its start, if it began.
    const {command, reason, turn_id} = event.CommandFailed;
    const notice = commandFailedNotice(turn_id, command, reason, envelope.context.sender_name, at, position);
    return chat(prev => notice.id ? replacing(prev, notice) : [...prev, notice]);
  }
  if (event.ParticipantJoined) {
    // Part of the history (issue 408), kept once by whom and when.
    const {participant, name} = event.ParticipantJoined;
    return chat(prev => once(prev, participantNotice('joined', participant, name, sender, envelope.context.sender_name, at)));
  }
  if (event.ParticipantLeft) {
    // Nothing of the participant that left follows (issue 162): what it had under way has
    // ended - its answer, its calls of tools. That it left is part of the history (issue 408).
    const {participant, name} = event.ParticipantLeft;
    const left = actorKey(participant);
    const notice = participantNotice('left', participant, name, sender, envelope.context.sender_name, at);
    // Its calls without a result as the chat shows them.
    const unanswered = new Set(mergeToolResults(messages)
      .filter(m => m.sender_id && sameActor(m.sender_id, participant) && m.tool_calls?.some(c => !m.tool_results?.some(r => r.id === c.id)))
      .map(m => m.id));
    const abandoned = messages.map(m => m.id && unanswered.has(m.id) ? {...m, abandoned: true} : m);
    return {
      ...conversation,
      messages: once(abandoned, notice),
      answers: answers.filter(a => a.sender !== left),
    };
  }
  if (event.GivenToRead) {
    // What came back from a thread (issue 557), at its place: part of the history, kept once by
    // the message's id.
    const given = givenBack(event.GivenToRead.message);
    return chat(prev => [...prev, ...given.filter(g => !prev.some(m => m.id === g.id))]);
  }
  if (event.ThreadOpened || event.ThreadClosed) {
    // A thread of the channel (issue 557).
    const change = event.ThreadOpened ? 'opened' : 'closed';
    const thread = (event.ThreadOpened ?? event.ThreadClosed)!.thread;
    return chat(prev => once(prev, threadNotice(change, thread, sender, envelope.context.sender_name, at)));
  }
  if (event.ChainCutOff) {
    const {max_agent_hops, not_asked} = event.ChainCutOff;
    return chat(prev => once(prev, chainCutOffNotice(max_agent_hops, not_asked, at, position)));
  }
  if (event.ResponderUndecided) {
    return chat(prev => once(prev, responderUndecidedNotice(at, position)));
  }
  if (event.NotDrawnIn) {
    return chat(prev => once(prev, notDrawnInNotice(event.NotDrawnIn!.mentioned, at, position)));
  }
  if (event.PlanStepStarted) {
    // Heads the messages of the step that follow (issue 263), as the history's mark of it does
    // after a reload.
    const {turn_id, ...step} = event.PlanStepStarted;
    return chat(prev => once(prev, planStepMark(turn_id, step, sender, envelope.context.sender_name, at)));
  }
  return conversation;
}
