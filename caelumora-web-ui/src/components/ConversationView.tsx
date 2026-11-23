// A conversation of a channel as the chat shows it (issue 595) - the line, or a thread: its
// messages, notices, steps of plans and answers under way, its earlier pages and the mark of what
// is new, with what the user may do with each message.

import React, {useEffect, useLayoutEffect, useMemo, useRef, useState} from 'react';
import {
  AlertTriangle,
  AtSign,
  Ban,
  BellRing,
  ChevronRight,
  Cog,
  MessagesSquare,
  Pencil,
  RotateCcw,
  ScrollText,
  Shield,
  Square,
  Undo2,
  Unlink,
  UserMinus,
  UserPlus,
} from 'lucide-react';
import type {ActorId, Message, Rejection, RetryRefused} from '../types';
import {labelOtherThan, type Classification} from '../confidentiality';
import {MessageEditor} from './MessageEditor';
import {cn} from '../classNames';
import {MarkdownMessage} from './MarkdownMessage';
import {MessageView, Reasoning} from './MessageView';
import {Notice} from './Notice';
import {Timestamp} from './Timestamp';
import {PlanSteps} from './PlanSteps';
import {attachedByTurn, chatRows, endedCommands, formatTimestamp, isOwn, joinsSteps, mayBeAnsweredAgain, mergeToolResults, rowTurn, turnGroups, withoutQuietCalls, type ChatRow} from '../messages';
import {actorKey, sameActor} from '../actor';
import type {Answer, Conversation} from '../conversation';
import {ShownChannelContext, useShownChannel, type ShownChannel} from '../shownChannel';
import {useChatChannel} from '../chatChannel';
import {useComposer} from '../composer';
import {Composer, StopButton} from './Composer';
import {ThreadMark} from './ThreadMark';
import type {UserInput} from '../command';
import {ApiRequestError, reportError} from '../errors';
import {isOthers} from '../unread';

/** Why an agent did not answer again (issue 234), as this page words it. */
function retryRefusedText(reason: RetryRefused): string {
  switch (reason) {
    case 'not_its_answer': return 'that is not an answer of its own it could give again';
    case 'not_last': return 'more was said or done since that answer';
    case 'calls_ran': return 'tools that already ran would be taken back with it';
    case 'asked_again': return 'it was asked again in the meantime';
    case 'elsewhere': return 'it runs elsewhere and cannot take back what it was given';
    case 'ended': return 'its answer had already ended';
    default: return 'for a reason this page does not know';
  }
}

/**
 * Why the channel did not let in what the user sent (issue 304), or the actor it went to did not
 * take it (issue 567), in words.
 */
function rejectionText(reason: Rejection): string {
  switch (reason) {
    case 'forbidden': return 'you may not do this here';
    case 'classification_refused': return 'it is classified so that the channel does not admit it';
    case 'message_not_found': return 'the message it is about is not in the channel';
    case 'message_unchangeable': return 'the message was taken back or cannot be changed so';
    case 'thread_closed': return 'the thread it is sent into is closed';
    case 'closed_circle': return 'only those it was opened with speak in the thread it is sent into';
    case 'invalid': return 'it does not fit the channel as it is, or what it is about is over';
    default: return 'for a reason this page does not know';
  }
}

/**
 * What the user sent, by its kind: what the channel did not let in (issue 304), or the actor it
 * went to did not take (issue 567).
 */
function sentText(kind: string): string {
  switch (kind) {
    case 'CompleteMessageGenerated': return 'Your message';
    case 'MessageModified': return 'Your change to a message';
    case 'MessageRevoked': return 'Taking a message back';
    case 'MessageReclassified': return 'The new classification of a message';
    case 'RetryRequested': return 'Your request for another answer';
    case 'QuestionAnswered': case 'QuestionDeclined': return 'Your answer to the questions';
    case 'CommandRequested': return 'Your command';
    case 'NudgeRequested': return 'Your request to respond';
    case 'ApprovalAnswered': return 'Your answer to an approval';
    default: return 'What you sent';
  }
}

/**
 * The heading of a command's notice: who ran it - or, for one of an agent (issue 269), which agent
 * carried it out or did not - and whether only you see it.
 */
function commandHeading(output: NonNullable<Message['commandOutput']>, sender: string | undefined) {
  const name = <span className="font-mono normal-case">/{output.command}{output.args ? ` ${output.args}` : ''}</span>;
  const yours = output.private ? ' · only visible to you' : '';
  if (output.failed) return <span>{output.agent} did not carry out {name}{yours}</span>;
  if (output.underWay) {
    return (
      <span className="inline-flex items-center gap-1.5">
        {output.agent} is carrying out {name}
        <span className="w-1.5 h-1.5 bg-orange-500 rounded-full animate-command" />
      </span>
    );
  }
  if (output.agent) return <span>{output.agent} carried out {name}{output.issuer ? ` for ${output.issuer}` : ''}{yours}</span>;
  return output.private
    ? <span>{name} · only visible to you</span>
    : <span>{sender} ran {name}</span>;
}

/**
 * Where the new messages of a conversation began as it was opened (issues 404, 595): after the
 * message last read - none if nothing was -, and how many came since.
 */
export interface NewSince {
  after: string | null;
  count: number;
}

interface ConversationViewProps {
  conversation: Conversation;
  /** Reads the page of the history before those shown. */
  onLoadEarlier: () => Promise<void>;
  /**
   * Where the new messages began as the conversation was opened (issue 404): after the message
   * last read, and how many came since.
   */
  newSince?: NewSince;
  /**
   * Marks the conversation read up to `message` (issues 404, 595): the newest message of someone
   * else it shows, while the tab is visible.
   */
  onRead: (message: string) => Promise<void>;
  /** Where what the user writes here is kept as a draft. */
  draftKey: string;
  /**
   * Sends `input` into the conversation - a message with what is `attached` to it, classified
   * `release`, or a command -; fails with why, and what was typed is given back.
   */
  onSend: (input: UserInput, attached: string[], release: Classification | null) => Promise<void>;
  /** Stops the answers under way in it; absent where nothing is offered to stop them all. */
  onStop?: () => void;
  /** Why the user cannot write here, said in place of the field to write in; absent if they can. */
  cannotWrite?: string;
  /**
   * Opens a thread at `message` (issue 595); offered at the messages of a conversation a person
   * opens threads in - the line.
   */
  onThreadAt?: (message: Message) => void;
  /** What stands between its list and the field to write in: what waits for the user there. */
  children?: React.ReactNode;
}

/**
 * A conversation as the chat shows it (issue 595): its list - the rows, the steps of plans and the
 * answers under way, the button for earlier pages and the mark of new messages, following its end
 * while the user is there -, what waits for the user, and the field to write in, onto which files
 * dropped are attached.
 */
export const ConversationView: React.FC<ConversationViewProps> = ({
  conversation, onLoadEarlier, newSince, onRead, draftKey, onSend, onStop, cannotWrite, onThreadAt, children,
}) => {
  const {
    me, channel, participants, takesPart, attachments, threads,
    retry, retryAnswer, retryGivenUp, modify, revoke, reclassify, showContext, interruptCall, stopAnswer,
  } = useChatChannel();
  const composer = useComposer(draftKey, attachments);
  // Whether files are dragged over the conversation, to be dropped as attachments.
  const [dragging, setDragging] = useState(false);
  const {answers, earlier} = conversation;
  const merged = useMemo(() => mergeToolResults(conversation.messages), [conversation.messages]);
  const shown = useMemo(() => withoutQuietCalls(merged), [merged]);
  // What its messages need of the channel, with what is the conversation's own (issues 294, 535):
  // the commands a result in it, or the user, ended, and what the messages of each turn carry.
  const outer = useShownChannel();
  const byUser = outer.endedCommands;
  const ended = useMemo(() => {
    const ids = endedCommands(merged);
    byUser?.forEach(id => ids.add(id));
    return ids;
  }, [merged, byUser]);
  const digestsByTurn = useMemo(() => attachedByTurn(shown), [shown]);

  // The newest message of someone else shown is what the user read (issue 404), marked while the
  // tab is visible: as the conversation is opened, as messages arrive and as the tab comes back.
  // Never one of their own, which is shown before the channel takes it in; nor a notice of the
  // UI's, which is in no history.
  const newestOfOthers = useMemo(() => shown.findLast(m =>
    m.id && (m.role === 'User' || m.role === 'Assistant') && isOthers(m, me)
    && !m.givenUp && !m.commandOutput && !m.activity && !m.chainCutOff && !m.nudged
  )?.id ?? null, [shown, me]);
  // Read through a ref, so that a caller's new function on every render marks nothing again.
  const read = useRef(onRead);
  useEffect(() => {
    read.current = onRead;
  });
  const marked = useRef<string | null>(null);
  useEffect(() => {
    if (!newestOfOthers) return;
    const mark = () => {
      if (document.visibilityState !== 'visible' || marked.current === newestOfOthers) return;
      marked.current = newestOfOthers;
      read.current(newestOfOthers).catch(error => {
        // A message the history does not hold marks nothing, and is not tried again.
        if (error instanceof ApiRequestError && error.status === 404) return;
        marked.current = null;
        reportError('Failed to mark what was read', error);
      });
    };
    mark();
    document.addEventListener('visibilitychange', mark);
    return () => document.removeEventListener('visibilitychange', mark);
  }, [newestOfOthers]);
  const shownChannel: ShownChannel = {...outer, endedCommands: ended, digestsByTurn};
  const [isLoadingEarlier, setIsLoadingEarlier] = useState(false);
  // The user's own message being edited, and the one whose taking back waits for a second click
  // (issue 234).
  const [editing, setEditing] = useState<string | null>(null);
  const [revoking, setRevoking] = useState<string | null>(null);
  // The turn whose messages are pointed at, shown together (issue 415).
  const [pointedTurn, setPointedTurn] = useState<string | null>(null);

  // The scrolling list, and whether it follows what is said: while the user is at its end, not
  // while they read further up.
  const listRef = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  // What the user says is followed, wherever they read.
  const send = (input: UserInput, attached: string[], release: Classification | null) => {
    following.current = true;
    return onSend(input, attached, release);
  };
  // At once, not smoothly: an answer streaming in grows with every token, and a scroll still under
  // way would read as the user's own.
  useLayoutEffect(() => {
    const list = listRef.current;
    if (list && following.current) list.scrollTop = list.scrollHeight;
  }, [conversation]);
  // Whether the user is at the end of the list, as they scroll.
  const scrolled = () => {
    const list = listRef.current;
    if (list) following.current = list.scrollHeight - list.scrollTop - list.clientHeight < 32;
  };

  const loadEarlier = async () => {
    setIsLoadingEarlier(true);
    try {
      await onLoadEarlier();
    } finally {
      setIsLoadingEarlier(false);
    }
  };

  /** Whether the user may change the classification of `msg`: its author or the channel's owner. */
  const mayReclassify = (msg: Message) =>
    !!msg.id && !!msg.classification && (!!channel?.can_change || isOwn(msg, me));

  /**
   * Whether `msg` is the user's own to edit and take back (issue 234): a stored message of theirs
   * not taken back, in a channel they take part in - only a participant has an actor to change it
   * through.
   */
  const mayChange = (msg: Message) => takesPart && !!msg.id && !msg.revoked && isOwn(msg, me);

  /**
   * Whether the user may ask `sender` to answer again: a participant whose `can_retry` allows it
   * (issue 242).
   */
  const mayRetryOf = (sender: string) => participants.some(m => actorKey(m.id) === sender && m.can_retry);

  /**
   * Whether the user may ask the agent that said `msg` to answer again (issue 234): the last
   * message of the conversation, of an agent that may be asked, while nothing is being generated.
   * The agent has the last word; this is an offer, not a promise.
   */
  const mayRetry = (msg: Message) => {
    if (!takesPart || answers.length > 0 || !msg.id || !msg.sender_id || !mayRetryOf(actorKey(msg.sender_id))) return false;
    const last = shown.findLast(m => m.id && !m.revoked);
    return last?.id === msg.id && !msg.internal;
  };

  /**
   * Whether the user may ask for an answer anew after the turn given up that the notice `msg` tells
   * of (issue 259), one that may be answered again, while it is the last thing in the
   * conversation - as `mayRetry` has it. The agent has the last word here, too.
   */
  const mayRetryGivenUp = (msg: Message) =>
    !!msg.turn_id && !!msg.givenUp && mayBeAnsweredAgain(msg.givenUp) && mayRetry(msg);

  /** The icon on the notice of a turn given up that asks for an answer anew, where it may. */
  const retryGivenUpAction = (msg: Message) => mayRetryGivenUp(msg) ? (
    <button
      onClick={() => void retryGivenUp(msg)}
      className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
      title="Ask for an answer anew"
      aria-label="Ask for an answer anew"
    >
      <RotateCcw size={13} />
    </button>
  ) : undefined;

  /** Takes `msg` back on the second click; the first asks for it. */
  const takeBack = async (msg: Message) => {
    if (revoking !== msg.id) {
      setRevoking(msg.id!);
      return;
    }
    setRevoking(null);
    await revoke(msg);
  };

  const saveEdit = async (msg: Message, content: string, carried: string[]) => {
    if (await modify(msg, content, carried)) setEditing(null);
  };

  /**
   * Whether the user may open a thread at `msg` (issue 595): a stored message said in the line -
   * not one of another thread that mentions them -, not taken back.
   */
  const mayThreadAt = (msg: Message) =>
    !!onThreadAt && takesPart && !!msg.id && !msg.revoked && !msg.internal
    && (msg.role === 'User' || msg.role === 'Assistant') && msg.thread === channel?.id;

  /**
   * Whether the user may see the context of `msg`: a stored message of an agent they may read
   * (issue 41).
   */
  const mayShowContext = (msg: Message): boolean => {
    if (!msg.id || !msg.sender_id) return false;
    const sender = msg.sender_id;
    return participants.some(m => sameActor(m.id, sender) && m.can_read_context);
  };

  /**
   * Whether the user may end the answer of `sender`: a participant whose `can_cancel` allows it
   * (issues 235, 243).
   */
  const mayCancelOf = (sender: string) => participants.some(m => actorKey(m.id) === sender && m.can_cancel);

  /**
   * Whether the user may interrupt a call of `msg` alone (issue 276): one of its turn, while its
   * agent runs tools and their `can_cancel` allows it. Only calls without a result show the offer.
   */
  const mayInterruptCallsOf = (msg: Message) => {
    if (!msg.turn_id || !msg.sender_id) return false;
    const sender = msg.sender_id;
    return participants.some(m => sameActor(m.id, sender) && m.can_cancel && m.state === 'ExecutingTools');
  };

  /** The participants `ids` name, by their handles where they have one, else `fallback`. */
  const namesOf = (ids: ActorId[], fallback: string) => ids.map(id => {
    const participant = participants.find(m => sameActor(m.id, id));
    return participant?.handle ? `@${participant.handle}` : (participant?.name ?? fallback);
  });

  /**
   * What a chain cut off at its limit says (issue 200), naming the agents mentioned as they are
   * known now.
   */
  const chainCutOffText = ({maxAgentHops, notAsked}: {maxAgentHops: number; notAsked: ActorId[]}): string => {
    const names = namesOf(notAsked, 'an agent');
    const whom = names.length > 0 ? `${names.join(', ')} ${names.length === 1 ? 'was' : 'were'} not asked` : 'no agent was asked';
    return `The chain between agents reached its limit of ${maxAgentHops} ${maxAgentHops === 1 ? 'hop' : 'hops'}, so ${whom} to respond. A message from a user starts a new chain.`;
  };

  /**
   * What a notice of whom a message could not bring into its thread says (issue 602), naming them
   * as they are known now.
   */
  const notDrawnInText = (mentioned: ActorId[]): string => {
    const names = namesOf(mentioned, 'someone');
    return `${names.join(', ')} cannot take part in this thread and ${names.length === 1 ? 'was' : 'were'} not brought in by your mention.`;
  };

  // One message of the conversation, as its kind shows it; `key` for the list it is rendered in,
  // `inTurn` where it stands among the messages of its turn shown together (issue 415).
  const renderMessage = (msg: Message, key: React.Key, inTurn?: {first: boolean; last: boolean}) => msg.participant ? (
    <div
      key={key}
      className="flex items-center justify-center gap-2 text-xs text-gray-500 dark:text-gray-400"
      title={formatTimestamp(msg.timestamp)?.full}
    >
      {msg.participant === 'joined' ? <UserPlus size={14} /> : <UserMinus size={14} />}
      <span>{msg.content}</span>
    </div>
  ) : msg.activity ? (
    <Notice
      key={key}
      message={msg}
      icon={Cog}
      heading={
        <span className="inline-flex items-center gap-1.5">
          {msg.activity.agent} {msg.activity.underWay ? 'is' : 'was'} {msg.activity.activity}
          {msg.activity.underWay && <span className="w-1.5 h-1.5 bg-green-500 rounded-full animate-working" />}
        </span>
      }
    >
      {msg.content && <MarkdownMessage content={msg.content} />}
    </Notice>
  ) : msg.commandOutput ? (
    <Notice key={key} message={msg} icon={ChevronRight} heading={commandHeading(msg.commandOutput, msg.sender_name)}>
      {msg.content && <MarkdownMessage content={msg.content} />}
    </Notice>
  ) : msg.nudged ? (
    <Notice key={key} message={msg} icon={BellRing} heading={<span>{msg.sender_name ?? 'Someone'} asks you to respond · only visible to you</span>} />
  ) : msg.chainCutOff ? (
    <Notice key={key} message={msg} icon={Unlink} heading={<span>Chain cut off</span>}>
      <div className="wrap-break-word">{chainCutOffText(msg.chainCutOff)}</div>
    </Notice>
  ) : msg.notDrawnIn ? (
    <Notice key={key} message={msg} icon={AtSign} heading={<span>Not brought into this thread · only visible to you</span>}>
      <div className="wrap-break-word">{notDrawnInText(msg.notDrawnIn.mentioned)}</div>
    </Notice>
  ) : msg.responderUndecided ? (
    <Notice key={key} message={msg} icon={AtSign} heading={<span>Nobody chosen to respond</span>}>
      <div className="wrap-break-word">It could not be decided who should respond, so no agent was asked. Mention someone with @ to ask them.</div>
    </Notice>
  ) : msg.givenUp?.kind === 'failed' ? (
    <Notice
      key={key}
      message={msg}
      icon={AlertTriangle}
      tone="alert"
      heading={msg.sender_name ? `${msg.sender_name} could not answer` : 'The turn failed'}
      actions={retryGivenUpAction(msg)}
    >
      <div className="wrap-break-word">{msg.content}</div>
      {msg.span_id && (
        <div className="mt-1 text-[11px] font-mono opacity-60">span_id {msg.span_id}</div>
      )}
    </Notice>
  ) : msg.givenUp ? (
    <Notice key={key} message={msg} icon={Ban} heading={<span>{msg.sender_name || 'The agent'} gave its answer up</span>} actions={retryGivenUpAction(msg)}>
      <div className="wrap-break-word">{msg.content}</div>
    </Notice>
  ) : msg.eventRejected ? (
    <Notice key={key} message={msg} icon={Ban} heading={<span>Not let in</span>}>
      <div className="wrap-break-word">{sentText(msg.eventRejected.rejected)} was not let into the channel: {rejectionText(msg.eventRejected.reason)}.</div>
    </Notice>
  ) : msg.eventRefused ? (
    <Notice key={key} message={msg} icon={Ban} heading={<span>Not taken</span>}>
      <div className="wrap-break-word">{sentText(msg.eventRefused.refused)} was not taken{msg.sender_name ? ` by ${msg.sender_name}` : ''}: {rejectionText(msg.eventRefused.reason)}.</div>
    </Notice>
  ) : msg.retryRefused ? (
    <Notice key={key} message={msg} icon={RotateCcw} heading={<span>Not answered again</span>}>
      <div className="wrap-break-word">{msg.sender_name || 'The agent'} did not answer again: {retryRefusedText(msg.retryRefused)}.</div>
    </Notice>
  ) : msg.rejected ? (
    <div
      key={key}
      className={cn(
        "max-w-[80%] p-3 rounded-lg border border-dashed border-red-300 dark:border-red-800 text-gray-600 dark:text-gray-300 text-sm",
        isOwn(msg, me) && "ml-auto",
      )}
    >
      <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1 opacity-70">
        <Ban size={14} />
        <span>Not let into the channel</span>
      </div>
      <div className="wrap-break-word whitespace-pre-wrap">{msg.content}</div>
      <div className="mt-1 text-[11px] opacity-70">{rejectionText(msg.rejected)}.</div>
    </div>
  ) : msg.revoked ? (
    <div
      key={key}
      className={cn(
        "max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 text-gray-500 dark:text-gray-400 text-sm italic",
        isOwn(msg, me) && "ml-auto",
      )}
    >
      {msg.sender_name || 'Its author'} took this message back
      <div className="mt-1 text-[11px] not-italic">
        <Timestamp timestamp={msg.revoked} className="opacity-70" />
      </div>
    </div>
  ) : editing === msg.id ? (
    <MessageEditor
      key={key}
      content={msg.content ?? ''}
      carried={msg.attachments ?? []}
      attachments={attachments}
      classification={msg.classification}
      onSave={(content, carried) => saveEdit(msg, content, carried)}
      onCancel={() => setEditing(null)}
    />
  ) : (
    <MessageView
      key={key}
      message={msg}
      me={me}
      participants={participants}
      onInterruptCall={mayInterruptCallsOf(msg) ? (call) => void interruptCall(msg, call) : undefined}
      inTurn={inTurn}
      badges={[channel && msg.classification ? labelOtherThan(msg.classification, channel.classification) : null]
        .flatMap(released => released ? [`released: ${released}`] : [])}
      actions={(mayShowContext(msg) || mayReclassify(msg) || mayChange(msg) || mayRetry(msg) || mayThreadAt(msg)) ? (
        <>
          {mayThreadAt(msg) && (
            <button
              onClick={() => onThreadAt?.(msg)}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title="Reply in a thread"
              aria-label="Reply in a thread"
            >
              <MessagesSquare size={13} />
            </button>
          )}
          {mayRetry(msg) && (
            <button
              onClick={() => void retry(msg)}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title="Ask for another answer instead of this one"
              aria-label="Ask for another answer instead of this one"
            >
              <RotateCcw size={13} />
            </button>
          )}
          {mayChange(msg) && (
            <button
              onClick={() => { setRevoking(null); setEditing(msg.id!); }}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title="Edit the message"
              aria-label="Edit the message"
            >
              <Pencil size={13} />
            </button>
          )}
          {mayChange(msg) && (
            <button
              onClick={() => void takeBack(msg)}
              onBlur={() => setRevoking(prev => prev === msg.id ? null : prev)}
              className={cn("p-0.5 flex items-center gap-1", revoking === msg.id ? "opacity-100 font-semibold" : "opacity-60 hover:opacity-100 focus:opacity-100")}
              title="Take the message back - it stays in the record of who saw it"
              aria-label={revoking === msg.id ? "Take the message back? Click again" : "Take the message back"}
            >
              <Undo2 size={13} />
              {revoking === msg.id && <span>Take back?</span>}
            </button>
          )}
          {mayReclassify(msg) && (
            <button
              onClick={() => reclassify(msg)}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title="Change the classification"
              aria-label="Change the classification"
            >
              <Shield size={13} />
            </button>
          )}
          {mayShowContext(msg) && (
            <button
              onClick={() => showContext(msg)}
              className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
              title="Show what the agent saw"
              aria-label="Show what the agent saw"
            >
              <ScrollText size={13} />
            </button>
          )}
        </>
      ) : undefined}
    />
  );

  // The threads that hang at each message shown (issue 595), marked below it.
  const hanging = new Map<string, typeof threads>();
  for (const thread of threads) {
    const at = thread.opening.origin.message;
    hanging.set(at, [...(hanging.get(at) ?? []), thread]);
  }
  // A message as `renderMessage` shows it, with the marks of the threads that hang at it.
  const withMarks = (msg: Message, key: React.Key, inTurn?: {first: boolean; last: boolean}) => {
    const marks = msg.id ? hanging.get(msg.id) : undefined;
    if (!marks) return renderMessage(msg, key, inTurn);
    return (
      <React.Fragment key={key}>
        {renderMessage(msg, 'message', inTurn)}
        {marks.map(thread => <ThreadMark key={thread.id} thread={thread} />)}
      </React.Fragment>
    );
  };

  const rows = chatRows(shown);
  // The row of each turn's step that came last: the step it is in now, open while the turn is
  // under way (issue 263).
  const currentSteps = new Map(rows.flatMap((row, i) => row.kind === 'steps' ? [[row.turnId, i] as const] : []));
  // An answer that streams in a step of a plan is shown where it will stand once complete: in the
  // step's block while nothing follows it (`joinsSteps`), else below - after whatever followed, or
  // where its turn has no block, a snapshot without the step's mark -, so that it does not move.
  const inStep = (answer: Answer) => answer.internal && joinsSteps(rows[rows.length - 1], answer.turnId);
  // The row before which the new messages begin (issue 404), if there were any as the
  // conversation was opened: after the one last read, or at the top where it lies before what is
  // shown or nothing was read.
  const newFrom = (() => {
    if (!newSince || newSince.count === 0 || rows.length === 0) return null;
    const read = newSince.after === null ? -1 : rows.findIndex(row => row.kind === 'message'
      ? row.message.id === newSince.after
      : row.messages.some(m => m.id === newSince.after));
    const from = read + 1;
    return from < rows.length ? from : null;
  })();
  // The rows grouped by turn (issue 415), and how many rows each turn has in all - also where
  // answers interleaved split it -, with its answer under way: the messages of a turn of more than
  // one are shown together when one of them is pointed at.
  const groups = turnGroups(rows, newFrom);
  const turnSizes = new Map<string, number>();
  for (const row of rows) {
    const turnId = rowTurn(row);
    if (turnId !== null) turnSizes.set(turnId, (turnSizes.get(turnId) ?? 0) + 1);
  }
  // An answer under way goes on with the last group where that is of its turn; any other is shown
  // below every group, with its name.
  const lastTurn = groups.length > 0 ? groups[groups.length - 1].turnId : null;
  const continuesLast = (answer: Answer) => !inStep(answer) && answer.turnId !== null && answer.turnId === lastTurn;
  if (lastTurn !== null) {
    const continuing = answers.filter(continuesLast).length;
    turnSizes.set(lastTurn, (turnSizes.get(lastTurn) ?? 0) + continuing);
  }
  // A row's key, stable while earlier pages are put in front of it.
  const rowKey = (row: ChatRow, idx: number) => row.kind === 'message'
    ? row.message.id ?? `local-${idx}`
    : `steps-${row.turnId}-${row.step?.step_index ?? ''}-${row.messages[0]?.id ?? idx}`;

  // An answer under way; `continues` where it follows the messages of its turn shown together,
  // which name the agent already (issue 415).
  const renderAnswer = (answer: Answer, continues = false) => (
    <div key={answer.sender} className="max-w-[80%] p-3 rounded-lg shadow-xs bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 rounded-bl-none border border-gray-200 dark:border-gray-600">
      <div className="text-xs font-semibold mb-1 opacity-70 uppercase tracking-wider flex items-center gap-2">
        {!continues && <span>{answer.name || 'Assistant'}</span>}
        <span className="inline-flex items-center gap-1.5 normal-case font-normal text-blue-500 dark:text-blue-400">
          <span className="w-1.5 h-1.5 bg-green-500 rounded-full animate-working" />
          generating…
        </span>
        <span className="ml-auto inline-flex items-center gap-1">
          {answer.turnId && mayRetryOf(answer.sender) && (
            <button
              type="button"
              onClick={() => void retryAnswer(answer)}
              className="p-0.5 rounded-sm text-gray-400 hover:text-blue-600 dark:hover:text-blue-400 transition-colors"
              title="Ask for another answer instead of this one"
              aria-label="Ask for another answer instead of this one"
            >
              <RotateCcw size={12} />
            </button>
          )}
          {mayCancelOf(answer.sender) && (
            <button
              type="button"
              onClick={() => void stopAnswer(answer.sender)}
              className="p-0.5 rounded-sm text-gray-400 hover:text-red-600 dark:hover:text-red-400 transition-colors"
              title="Stop this answer alone"
              aria-label="Stop this answer alone"
            >
              <Square size={12} fill="currentColor" />
            </button>
          )}
        </span>
      </div>
      {answer.reasoning && <Reasoning text={answer.reasoning} thinking={!answer.content} />}
      {answer.content && <MarkdownMessage content={answer.content} />}
    </div>
  );

  return (
    <ShownChannelContext value={shownChannel}>
      <div
        // Files dropped onto the conversation are attached to the message written (issue 282).
        onDragOver={(e) => {
          if (cannotWrite || !e.dataTransfer.types.includes('Files')) return;
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={(e) => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false); }}
        onDrop={(e) => {
          if (cannotWrite || e.dataTransfer.files.length === 0) return;
          e.preventDefault();
          setDragging(false);
          composer.draft.add([...e.dataTransfer.files], composer.release ?? undefined);
        }}
        className={cn("flex-1 flex flex-col min-w-0", dragging && "ring-2 ring-inset ring-blue-500")}
      >
        <div ref={listRef} onScroll={scrolled} className="flex-1 overflow-y-auto p-4 space-y-4">
          {earlier !== null && (
            <div className="text-center">
              <button
                onClick={() => void loadEarlier()}
                disabled={isLoadingEarlier}
                className="text-xs px-3 py-1 rounded-md border border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-800 disabled:opacity-50"
              >
                {isLoadingEarlier ? 'Loading…' : 'Load earlier messages'}
              </button>
            </div>
          )}
          {groups.map(group => {
            const trailing = group === groups[groups.length - 1] ? answers.filter(continuesLast) : [];
            const together = group.turnId !== null && (turnSizes.get(group.turnId) ?? 0) > 1;
            return [group.start === newFrom && (
              <div key="new-messages" role="separator" aria-label="New messages" className="flex items-center gap-2 text-xs font-semibold text-red-600 dark:text-red-400">
                <div className="flex-1 border-t border-red-400 dark:border-red-500" />
                New messages
                <div className="flex-1 border-t border-red-400 dark:border-red-500" />
              </div>
            ), (
              <div
                key={`turn-${rowKey(rows[group.start], group.start)}`}
                onMouseEnter={together ? () => setPointedTurn(group.turnId) : undefined}
                onMouseLeave={together ? () => setPointedTurn(null) : undefined}
                className={cn(
                  "space-y-1",
                  // Indented as a whole, its frame flush with the messages alone and as far from
                  // its messages as they are from their content.
                  together && "p-3 rounded-lg transition-colors",
                  together && pointedTurn === group.turnId && "bg-gray-200/70 dark:bg-gray-700/50 ring-1 ring-gray-300 dark:ring-gray-600",
                )}
              >
                {rows.slice(group.start, group.end).map((row, offset) => {
                  const idx = group.start + offset;
                  const inTurn = {first: idx === group.start, last: idx === group.end - 1 && trailing.length === 0};
                  if (row.kind === 'message') return withMarks(row.message, rowKey(row, idx), inTurn);
                  const current = currentSteps.get(row.turnId) === idx;
                  const streaming = current ? answers.filter(a => a.turnId === row.turnId && inStep(a)) : [];
                  return (
                    <PlanSteps
                      key={rowKey(row, idx)}
                      name={row.sender || 'Assistant'}
                      step={row.step}
                      count={row.messages.length}
                      underWay={row.turnId !== null && current && answers.some(a => a.turnId === row.turnId)}
                      streaming={streaming.length > 0}
                    >
                      {row.messages.map((msg, i) => withMarks(msg, msg.id ?? `local-${i}`, {first: false, last: i === row.messages.length - 1 && streaming.length === 0}))}
                      {streaming.map(answer => renderAnswer(answer, true))}
                    </PlanSteps>
                  );
                })}
                {trailing.map(answer => renderAnswer(answer, true))}
              </div>
            )];
          })}
          {answers.filter(answer => !inStep(answer) && !continuesLast(answer)).map(answer => renderAnswer(answer))}
        </div>
        {children}
        <div className="p-4 bg-white dark:bg-gray-900 border-t border-gray-200 dark:border-gray-700">
          {channel?.unavailable && (
            <p className="text-sm text-amber-700 dark:text-amber-400 text-center p-2">
              This channel does not run: {channel.unavailable}. It runs again once its components can be read - whatever you do here tries again -; its owner can delete it.
            </p>
          )}
          {cannotWrite ? (
            // Who may not write here may still stop what is generated (issue 595): a sub-agent's.
            <div className="flex items-center gap-2">
              <p className="flex-1 text-sm text-gray-500 dark:text-gray-400 text-center p-2">{cannotWrite}</p>
              {answers.length > 0 && onStop && <StopButton onStop={onStop} />}
            </div>
          ) : (
            <Composer composer={composer} onSend={send} onStop={onStop} generating={answers.length > 0} />
          )}
        </div>
      </div>
    </ShownChannelContext>
  );
};
