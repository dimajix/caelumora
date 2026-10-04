import React, {useEffect, useRef, useState} from 'react';
import {
  AlertTriangle,
  ChevronRight,
  NotebookPen,
  Pencil,
  Scale,
  Shield,
  ScrollText,
  Send,
  Square,
  UserMinus,
  UserPlus,
  Users,
  Wrench,
} from 'lucide-react';
import type {
  ApprovalRequest,
  ChannelMember,
  ChannelResponse,
  ChannelSnapshot,
  Command,
  Compartment,
  Envelope,
  Message,
  PlanStepEvent,
  PublishedNotes,
  UserResponse,
} from '../types';
import {NotesList} from './NotesList';
import {answersOnlyWhenMentioned, responderPolicyInfo} from '../moderator';
import {
  CONFIDENTIALITY_LEVELS,
  type Classification,
  classificationDescription,
  classificationLabel,
  levelsUpTo,
  sameClassification,
} from '../confidentiality';
import {ClassificationPicker} from './ClassificationPicker';
import {ClassificationDialog} from './ClassificationDialog';
import {changedChannel, membershipChangedChannel, onGlobalEvent, reclassifiedChannel, reclassifiedMessage} from '../globalEvents';
import {api} from '../api';
import {ApiRequestError, reportError} from '../errors';
import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';
import {MarkdownMessage} from './MarkdownMessage';
import {MessageFooter, MessageView, Reasoning} from './MessageView';
import {formatArguments, mergeToolResults} from '../messages';
import {ContextPanel} from './ContextPanel';
import {AddParticipantDialog} from './AddParticipantDialog';
import {actorKey, actorUuid, handleOf, mentionQueryAt} from '../mention';
import {commandQueryAt, parseInput} from '../command';
import {openEventStream} from '../stream';
import {loadDraft, saveDraft} from '../session';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

type MentionableMember = ChannelMember & {handle: string};

/** An entry of the completion list: a member to mention or a command to run. */
type Suggestion =
  | {kind: 'mention'; member: MentionableMember}
  | {kind: 'command'; command: Command};

/**
 * An agent's answer under way (issue 203): what of it has streamed in since
 * its last complete message, and the plan step it is at. Several agents can
 * answer at once, each with an answer of its own.
 */
interface Answer {
  /** Its sender, as `actorKey`. */
  sender: string;
  name: string | null;
  reasoning: string;
  content: string;
  step: PlanStepEvent | null;
}

/**
 * `answers` with the answer of `sender` changed by `change` - added at the
 * end if it has none, so that answers keep the order they began in.
 */
function withAnswer(answers: Answer[], sender: string, name: string | null, change: (answer: Answer) => Answer): Answer[] {
  const index = answers.findIndex(a => a.sender === sender);
  if (index < 0) {
    return [...answers, change({sender, name, reasoning: '', content: '', step: null})];
  }
  return answers.map((a, i) => i === index ? change({...a, name: name ?? a.name}) : a);
}

interface ChatViewProps {
  sessionId: string;
  channelId: string;
  /** Who is logged in: only a participant follows the channel live. */
  user: UserResponse;
}

export const ChatView: React.FC<ChatViewProps> = ({ sessionId, channelId, user }) => {
  const [messages, setMessages] = useState<Message[]>([]);
  const [channel, setChannel] = useState<ChannelResponse | null>(null);
  const [notes, setNotes] = useState<PublishedNotes[]>([]);
  const [sideTab, setSideTab] = useState<'members' | 'notes'>('members');
  // Restored from session storage, so a forced re-login does not lose it.
  const [inputText, setInputText] = useState(() => loadDraft(channelId));
  const [isSending, setIsSending] = useState(false);
  // How the next message is classified, in the channel it was chosen in;
  // `null`: as the channel (issue 133).
  const [chosenRelease, setChosenRelease] = useState<{channelId: string; classification: Classification} | null>(null);
  const release = chosenRelease?.channelId === channelId ? chosenRelease.classification : null;
  const setRelease = (classification: Classification | null) =>
    setChosenRelease(classification && {channelId, classification});
  const [compartments, setCompartments] = useState<Compartment[]>([]);
  // The answers under way, in the order they began (issue 203). Shown below
  // every complete message: a message joins the history when it is complete,
  // so an answer that ends first is shown first, as a reload shows it.
  const [answers, setAnswers] = useState<Answer[]>([]);
  const isGenerating = answers.length > 0;
  const [pendingApprovals, setPendingApprovals] = useState<ApprovalRequest[]>([]);
  // The pending approvals this user answered, still waiting for the decision.
  const [answeredApprovals, setAnsweredApprovals] = useState<string[]>([]);
  // The snapshot holds the end of the history; older messages on request.
  const [hasMore, setHasMore] = useState(false);
  const [isLoadingHistory, setIsLoadingHistory] = useState(false);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  // `@`-completion: the caret position the suggestions were computed for, the
  // highlighted suggestion, and the query the user dismissed with Escape.
  const [caret, setCaret] = useState(0);
  const [suggestionIndex, setSuggestionIndex] = useState(0);
  const [dismissedMention, setDismissedMention] = useState<number | null>(null);
  // The agent message whose context is shown, with the channel it belongs
  // to - another channel shows none (issue 41).
  const [contextOf, setContextOf] = useState<{channelId: string; message: Message} | null>(null);
  // The commands this user may run here, for `/` completion (issue 28).
  const [commands, setCommands] = useState<Command[]>([]);
  // Whether the dialog to add a participant is open (issue 46).
  const [isAddingParticipant, setIsAddingParticipant] = useState(false);
  // What is being reclassified (issue 133): the channel, or one message.
  const [reclassifying, setReclassifying] = useState<{kind: 'channel'} | {kind: 'message'; message: Message} | null>(null);

  // The server returns members in no particular order.
  const members: ChannelMember[] = [...(channel?.members ?? [])].sort((l, r) => l.name.localeCompare(r.name));
  const mentionQuery = mentionQueryAt(inputText, caret);
  const commandQuery = commandQueryAt(inputText, caret);
  // Where the completed token starts; a command always starts at 0.
  const suggestionStart = commandQuery !== null ? 0 : mentionQuery?.start ?? null;
  const suggestions: Suggestion[] = suggestionStart === null || suggestionStart === dismissedMention
    ? []
    : commandQuery !== null
      ? commands
          .filter(c => c.name.startsWith(commandQuery.toLowerCase()))
          .map(command => ({kind: 'command', command}))
      : members
          .filter((m): m is MentionableMember => {
            const query = (mentionQuery?.query ?? '').toLowerCase();
            return !!m.handle && (m.handle.startsWith(handleOf(query)) || m.name.toLowerCase().includes(query));
          })
          .map(member => ({kind: 'mention', member}));

  // Whether the logged-in user takes part in this channel; `undefined` until
  // the channel is known. Joining or leaving changes it (the members follow
  // the global stream), and the view follows.
  const participant = channel?.id === channelId
    ? channel.members.some(m => typeof m.id !== 'string' && m.id.User === user.id)
    : undefined;

  // The compartments a message may be released to; none if they cannot be
  // fetched - the message is then the channel's.
  useEffect(() => {
    api.getCompartments()
      .then(setCompartments)
      .catch(error => reportError('Failed to load the compartments', error));
  }, []);

  useEffect(() => {
    api.getCommands(sessionId, channelId)
      .then(setCommands)
      .catch(error => {
        // Someone who is not a participant has no commands here - and learns
        // that when writing; completion is only a help and stays silent.
        if (error instanceof ApiRequestError && error.status === 403) return;
        reportError('Failed to load the commands', error);
      });
  }, [sessionId, channelId]);

  // The notes follow the global stream: a changed state of an agent in this
  // channel is announced without content, and the notes are fetched again
  // (issue 65).
  useEffect(() => {
    return onGlobalEvent(event => {
      if (changedChannel(event) !== channelId) return;
      api.getNotes(sessionId, channelId)
        .then(setNotes)
        .catch(error => reportError('Failed to load the notes', error));
    });
  }, [sessionId, channelId]);

  // The members follow the global stream as well: someone joined or left, and
  // the channel is fetched again (issue 46). Whoever was removed can no
  // longer fetch it and keeps what they saw until they leave the channel.
  useEffect(() => {
    return onGlobalEvent(event => {
      if (membershipChangedChannel(event) !== channelId) return;
      api.getChannel(sessionId, channelId)
        .then(setChannel)
        .catch(error => {
          if (error instanceof ApiRequestError && error.status === 404) return;
          reportError('Failed to load the channel', error);
        });
    });
  }, [sessionId, channelId]);

  // Reclassifications follow the global stream (issue 133): a message's
  // classification is taken from the event; a reclassified channel may have
  // changed its messages too, which are fetched again.
  useEffect(() => {
    return onGlobalEvent(event => {
      const message = reclassifiedMessage(event);
      if (message && message.channelId === channelId) {
        setMessages(prev => prev.map(m => m.id === message.messageId ? {...m, classification: message.classification} : m));
        return;
      }
      const reclassified = reclassifiedChannel(event);
      if (!reclassified || reclassified.channelId !== channelId) return;
      setChannel(prev => prev && {...prev, classification: reclassified.classification});
      api.getMessages(sessionId, channelId)
        .then(data => {
          const now = new Map(data.map(m => [m.id, m.classification]));
          setMessages(prev => prev.map(m => (m.id && now.get(m.id)) ? {...m, classification: now.get(m.id)} : m));
        })
        .catch(error => reportError('Failed to load the messages', error));
    });
  }, [sessionId, channelId]);

  /** Whether the user may change the classification of `msg`: its author, or the channel's owner. */
  const mayReclassify = (msg: Message) =>
    !!msg.id && !!msg.classification && (!!channel?.can_change || (typeof msg.sender_id === 'object' && msg.sender_id !== null && 'User' in msg.sender_id && msg.sender_id.User === user.id));

  const removeParticipant = async (member: ChannelMember) => {
    const uuid = actorUuid(member.id);
    if (!uuid) return;
    try {
      await api.removeParticipant(sessionId, channelId, uuid);
      setChannel(prev => prev && {...prev, members: prev.members.filter(m => actorKey(m.id) !== actorKey(member.id))});
    } catch (error) {
      reportError(`Failed to remove ${member.name}`, error);
    }
  };

  useEffect(() => {
    const handleSnapshot = (data: string) => {
      try {
        const snapshot: ChannelSnapshot = JSON.parse(data);
        setMessages(snapshot.messages);
        setChannel(snapshot.channel);
        setNotes(snapshot.notes ?? []);
        setPendingApprovals(snapshot.pending_approvals ?? []);
        setHasMore(snapshot.has_more);
        setAnswers([]);
      } catch (e) {
        reportError('Failed to read the channel state', e);
      }
    };

    const handleMessage = (data: string) => {
      try {
        const envelope: Envelope = JSON.parse(data);
        const actorEvent = envelope.event;
        const answerer = actorKey(envelope.context.sender_id);
        const answererName = envelope.context.sender_name || null;
        // The answer of the envelope's sender, if it has one, has ended.
        const answerEnded = () => setAnswers(prev => prev.filter(a => a.sender !== answerer));
        if (actorEvent === 'ResponseStarted') {
          // A new answer begins after those under way.
          setAnswers(prev => withAnswer(prev.filter(a => a.sender !== answerer), answerer, answererName, a => a));
        } else if (typeof actorEvent === 'string') {
          // The other payload-less events have nothing to show.
        } else if (actorEvent.PartialMessageGenerated) {
          const partial = actorEvent.PartialMessageGenerated;
          setAnswers(prev => withAnswer(prev, answerer, answererName, a => ({
            ...a,
            reasoning: a.reasoning + (partial.reasoning ?? ''),
            content: a.content + (partial.content ?? ''),
          })));
        } else if (actorEvent.CompleteMessageGenerated) {
          setMessages(prev => {
            const newMsg = actorEvent.CompleteMessageGenerated!;
            // A message can arrive twice: in the snapshot and as an event.
            if (newMsg.id && prev.some(m => m.id === newMsg.id)) return prev;
            if (newMsg.tool_results && newMsg.tool_results.length > 0) {
              const lastMsg = prev[prev.length - 1];
              if (lastMsg && lastMsg.tool_calls && lastMsg.tool_calls.some(tc => newMsg.tool_results?.some(tr => tr.id === tc.id))) {
                const updatedLastMsg = {
                  ...lastMsg,
                  tool_results: [...(lastMsg.tool_results || []), ...newMsg.tool_results]
                };
                return [...prev.slice(0, -1), updatedLastMsg];
              }
            }
            return [...prev, newMsg];
          });
          // Its sender may go on: what streams in next is its next message.
          setAnswers(prev => prev.map(a => a.sender === answerer ? {...a, reasoning: '', content: ''} : a));
        } else if (actorEvent.ApprovalRequested) {
          // Asked again by an agent that resumed its turn (issue 172), or
          // already in the snapshot: shown once.
          const requested = actorEvent.ApprovalRequested;
          setPendingApprovals(prev => prev.some(req => req.id === requested.id) ? prev : [...prev, requested]);
        } else if (actorEvent.ApprovalGranted || actorEvent.ApprovalDenied || actorEvent.ApprovalTimedOut || actorEvent.ApprovalRequestWithdrawn) {
          // Over only once the approver says so - decided, or no longer
          // asked for (issue 148); a user's answer alone may not decide it.
          const sender = envelope.context.sender_id;
          const ended = (actorEvent.ApprovalGranted ?? actorEvent.ApprovalDenied ?? actorEvent.ApprovalTimedOut ?? actorEvent.ApprovalRequestWithdrawn)!.id;
          if (typeof sender === 'object' && 'Approver' in sender) {
            setPendingApprovals(prev => prev.filter(req => req.id !== ended));
            setAnsweredApprovals(prev => prev.filter(id => id !== ended));
          }
        } else if (actorEvent.FullResponseGenerated) {
          //setMessages(prev => [...prev, ...actorEvent.FullResponseGenerated!]);
          answerEnded();
        } else if (actorEvent.TurnFailed) {
          // Shown in the chat, but not part of the history: gone on reload.
          const failed: Message = {
            role: 'System',
            content: actorEvent.TurnFailed.message,
            sender_name: envelope.context.sender_name,
            turnFailed: true,
            span_id: envelope.context.span_id,
            timestamp: envelope.timestamp,
          };
          setMessages(prev => [...prev, failed]);
          answerEnded();
        } else if (actorEvent.CommandExecuted) {
          // Like a failed turn: shown, but not part of the history.
          const notice: Message = {
            role: 'System',
            content: actorEvent.CommandExecuted.output,
            sender_name: envelope.context.sender_name,
            commandOutput: {command: actorEvent.CommandExecuted.command, private: false},
            timestamp: envelope.timestamp,
          };
          setMessages(prev => [...prev, notice]);
        } else if (actorEvent.ParticipantRemoved) {
          // Nothing of the removed participant follows (issue 162): what it
          // had under way has ended - its answer, its calls of tools.
          const removed = actorKey(actorEvent.ParticipantRemoved.participant);
          setMessages(prev => prev.map(m =>
            m.sender_id && actorKey(m.sender_id) === removed && m.tool_calls?.some(c => !m.tool_results?.some(r => r.id === c.id))
              ? {...m, abandoned: true}
              : m
          ));
          setAnswers(prev => prev.filter(a => a.sender !== removed));
        } else if (actorEvent.PlanStepStarted) {
          const step = actorEvent.PlanStepStarted;
          setAnswers(prev => withAnswer(prev, answerer, answererName, a => ({...a, step})));
        } else if (actorEvent.PlanStepFinished) {
          const finished = actorEvent.PlanStepFinished.step_index;
          setAnswers(prev => prev.map(a =>
            a.sender === answerer && a.step?.step_index === finished ? {...a, step: null} : a
          ));
        }
      } catch (e) {
        console.error('Failed to parse SSE event', e);
      }
    };

    // Not known yet: the channel is fetched first, and this runs again.
    if (participant === undefined) {
      let cancelled = false;
      api.getChannel(sessionId, channelId)
        .then(found => { if (!cancelled) setChannel(found); })
        .catch(error => reportError('Failed to load the channel', error));
      return () => { cancelled = true; };
    }
    // Only a participant follows a channel live, through their own actor
    // (issue 104); anyone else who may read it sees its history as it is.
    if (!participant) {
      let cancelled = false;
      api.getMessages(sessionId, channelId)
        .then(history => { if (!cancelled) setMessages(history); })
        .catch(error => reportError('Failed to load the messages', error));
      return () => { cancelled = true; };
    }
    // The stream itself delivers the state it starts from, so there is no
    // window between loading the history and subscribing in which a message
    // could be missed (issue 34).
    return openEventStream(api.channelEventsUrl(sessionId, channelId), {
      onSnapshot: handleSnapshot,
      onEvent: handleMessage,
    });
  }, [sessionId, channelId, participant]);


  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, answers]);

  useEffect(() => {
    saveDraft(channelId, inputText);
  }, [channelId, inputText]);

  /** Replaces the snapshot's tail with the channel's full history. */
  const loadFullHistory = async () => {
    setIsLoadingHistory(true);
    try {
      const data = await api.getMessages(sessionId, channelId);
      setMessages(prev => {
        // Keep anything that arrived after the history was read.
        const known = new Set(data.map(m => m.id));
        return [...data, ...prev.filter(m => !m.id || !known.has(m.id))];
      });
      setHasMore(false);
    } catch (error) {
      reportError('Failed to load earlier messages', error);
    } finally {
      setIsLoadingHistory(false);
    }
  };

  const handleSendMessage = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!inputText.trim() || isSending) return;

    const text = inputText;
    setInputText('');
    setIsSending(true);

    // Optimistic update
    //const userMsg: Message = { role: 'User', content: text };
    //setMessages(prev => [...prev, userMsg]);

    const input = parseInput(text);
    try {
      if (input.kind === 'command') {
        const reply = await api.executeCommand(sessionId, channelId, input.name, input.args);
        // An answer for the whole channel arrives on the stream instead.
        if (reply.output !== null) {
          const notice: Message = {
            role: 'System',
            content: reply.output,
            commandOutput: {command: reply.command, private: true},
            // Private output has no envelope: the time it arrived.
            timestamp: new Date().toISOString(),
          };
          setMessages(prev => [...prev, notice]);
        }
      } else {
        await api.sendMessage(sessionId, channelId, input.text, release ?? undefined);
        // A release is for one message.
        setRelease(null);
      }
    } catch (error) {
      // The server's detail for a command already names it.
      reportError(input.kind === 'command' ? 'Command failed' : 'Failed to send message', error);
      // Give the text back rather than losing it.
      setInputText(prev => prev || text);
    } finally {
      setIsSending(false);
    }
  };

  const updateInput = (e: React.ChangeEvent<HTMLInputElement>) => {
    setInputText(e.target.value);
    setCaret(e.target.selectionStart ?? e.target.value.length);
    setSuggestionIndex(0);
  };

  const selectCommand = (command: Command) => {
    const rest = inputText.slice(caret).replace(/^\S*\s*/, '');
    const inserted = `/${command.name} `;
    const newCaret = inserted.length;
    setInputText(inserted + rest);
    setCaret(newCaret);
    setSuggestionIndex(0);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.setSelectionRange(newCaret, newCaret);
    });
  };

  const selectSuggestion = (suggestion: Suggestion) => {
    if (suggestion.kind === 'command') selectCommand(suggestion.command);
    else selectMention(suggestion.member);
  };

  const selectMention = (member: MentionableMember) => {
    if (!mentionQuery) return;
    const before = inputText.slice(0, mentionQuery.start);
    const after = inputText.slice(caret).replace(/^\s*/, ' ');
    const inserted = `@${member.handle}`;
    const newCaret = before.length + inserted.length + 1;
    setInputText(before + inserted + after);
    setCaret(newCaret);
    setSuggestionIndex(0);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.setSelectionRange(newCaret, newCaret);
    });
  };

  const handleInputKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (suggestions.length === 0) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSuggestionIndex(i => (i + 1) % suggestions.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSuggestionIndex(i => (i - 1 + suggestions.length) % suggestions.length);
    } else if (e.key === 'Enter' || e.key === 'Tab') {
      e.preventDefault();
      selectSuggestion(suggestions[Math.min(suggestionIndex, suggestions.length - 1)]);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      setDismissedMention(suggestionStart);
    }
  };

  /** Whether the user may see the context of `msg`: a stored message of an agent they may read (issue 41). */
  const mayShowContext = (msg: Message): boolean => {
    if (!msg.id || !msg.sender_id) return false;
    const sender = actorKey(msg.sender_id);
    return members.some(m => actorKey(m.id) === sender && m.can_read_context);
  };

  const handleStopGeneration = async () => {
    try {
      await api.cancelChannel(sessionId, channelId);
    } catch (error) {
      reportError('Failed to cancel generation', error);
    }
  };

  const handleApproval = async (approval: ApprovalRequest, approved: boolean) => {
    try {
      await api.respondToApproval(sessionId, channelId, approval.id, approval.agent_id, approved);
      setAnsweredApprovals(prev => [...prev, approval.id]);
    } catch (e) {
      reportError('Failed to respond to approval', e);
    }
  };

  return (
    <div className="flex-1 flex overflow-hidden bg-gray-50 dark:bg-gray-800">
      <div className="flex-1 flex flex-col min-w-0">
        {answers.map(answer => answer.step && (
          <div key={answer.sender} className="px-4 py-2 bg-blue-50 dark:bg-blue-900/20 border-b border-blue-200 dark:border-blue-900/50 flex items-center gap-2">
            <div className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse shrink-0" />
            <span className="text-xs font-semibold uppercase tracking-wider text-blue-700 dark:text-blue-300 shrink-0">
              {answer.name || 'Assistant'} · Step {answer.step.step_index + 1} of {answer.step.total_steps}
            </span>
            <span className="text-xs text-blue-700 dark:text-blue-300 truncate">
              {answer.step.description}
            </span>
          </div>
        ))}
        <div className="flex-1 overflow-y-auto p-4 space-y-4">
          {hasMore && (
            <div className="text-center">
              <button
                onClick={loadFullHistory}
                disabled={isLoadingHistory}
                className="text-xs px-3 py-1 rounded-md border border-gray-300 dark:border-gray-600 text-gray-600 dark:text-gray-300 hover:bg-gray-100 dark:hover:bg-gray-800 disabled:opacity-50"
              >
                {isLoadingHistory ? 'Loading…' : 'Load earlier messages'}
              </button>
            </div>
          )}
          {mergeToolResults(messages).map((msg, idx) => msg.commandOutput ? (
            <div
              key={idx}
              className="max-w-[80%] p-3 rounded-lg border border-dashed border-gray-300 dark:border-gray-600 bg-gray-100 dark:bg-gray-900/60 text-gray-800 dark:text-gray-200 text-sm"
            >
              <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1 opacity-70">
                <ChevronRight size={14} />
                {msg.commandOutput.private
                  ? <span><span className="font-mono normal-case">/{msg.commandOutput.command}</span> · only visible to you</span>
                  : <span>{msg.sender_name} ran <span className="font-mono normal-case">/{msg.commandOutput.command}</span></span>}
              </div>
              <MarkdownMessage content={msg.content ?? ''} />
              <MessageFooter message={msg} />
            </div>
          ) : msg.turnFailed ? (
            <div
              key={idx}
              role="alert"
              className="max-w-[80%] p-3 rounded-lg border border-red-300 dark:border-red-800 bg-red-50 dark:bg-red-900/30 text-red-800 dark:text-red-200 text-sm"
            >
              <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider mb-1">
                <AlertTriangle size={14} />
                {msg.sender_name ? `${msg.sender_name} could not answer` : 'The turn failed'}
              </div>
              <div className="wrap-break-word">{msg.content}</div>
              {msg.span_id && (
                <div className="mt-1 text-[11px] font-mono opacity-60">span_id {msg.span_id}</div>
              )}
              <MessageFooter message={msg} />
            </div>
          ) : (
            <MessageView
              key={idx}
              message={msg}
              members={members}
              badges={channel && msg.classification && !sameClassification(msg.classification, channel.classification)
                ? [`released: ${classificationLabel(msg.classification)}`]
                : undefined}
              actions={(mayShowContext(msg) || mayReclassify(msg)) ? (
                <>
                  {mayReclassify(msg) && (
                    <button
                      onClick={() => setReclassifying({kind: 'message', message: msg})}
                      className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
                      title="Change the classification"
                      aria-label="Change the classification"
                    >
                      <Shield size={13} />
                    </button>
                  )}
                  {mayShowContext(msg) && (
                    <button
                      onClick={() => setContextOf({channelId, message: msg})}
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
          ))}
          {answers.map(answer => (
            <div key={answer.sender} className="max-w-[80%] p-3 rounded-lg shadow-xs bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 rounded-bl-none border border-gray-200 dark:border-gray-600">
              <div className="text-xs font-semibold mb-1 opacity-70 uppercase tracking-wider flex items-center gap-2">
                <span>{answer.name || 'Assistant'}</span>
                <span className="inline-flex items-center gap-1.5 normal-case font-normal text-blue-500 dark:text-blue-400">
                  <span className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse" />
                  generating…
                </span>
              </div>
              {answer.reasoning && <Reasoning text={answer.reasoning} thinking={!answer.content} />}
              {answer.content && <MarkdownMessage content={answer.content} />}
            </div>
          ))}
          <div ref={messagesEndRef} />
        </div>

        {pendingApprovals.length > 0 && (
          <div className="p-4 bg-yellow-50 dark:bg-yellow-900/20 border-t border-yellow-200 dark:border-yellow-900/50 space-y-4">
            <h4 className="text-sm font-semibold text-yellow-800 dark:text-yellow-200 flex items-center gap-2">
              <Wrench size={16} />
              Tool Execution Approval Required
            </h4>
            {pendingApprovals.map(approval => (
              <div key={approval.id} className="bg-white dark:bg-gray-800 rounded-lg border border-yellow-200 dark:border-yellow-900/50 shadow-xs p-4">
                <div className="flex items-center justify-between mb-2">
                   <div className="text-xs font-mono font-bold text-gray-500">{approval.tool_call.function_name}</div>
                   <div className="text-[11px] text-gray-400">ID: {approval.id}</div>
                </div>
                <pre className="text-xs font-mono bg-gray-50 dark:bg-gray-900 p-2 rounded-sm mb-4 overflow-x-auto">
                  {formatArguments(approval.tool_call.arguments)}
                </pre>
                {answeredApprovals.includes(approval.id) ? (
                  <p className="text-sm text-gray-500 dark:text-gray-400 text-center">Answered - waiting for the decision.</p>
                ) : (
                <div className="flex gap-2">
                  <button
                    onClick={() => handleApproval(approval, true)}
                    className="flex-1 bg-green-600 hover:bg-green-700 text-white text-sm py-2 rounded-md transition-colors"
                  >
                    Approve
                  </button>
                  <button
                    onClick={() => handleApproval(approval, false)}
                    className="flex-1 bg-red-600 hover:bg-red-700 text-white text-sm py-2 rounded-md transition-colors"
                  >
                    Deny
                  </button>
                </div>
                )}
              </div>
            ))}
          </div>
        )}

        <div className="p-4 bg-white dark:bg-gray-900 border-t border-gray-200 dark:border-gray-700">
          {participant === false ? (
            <p className="text-sm text-gray-500 dark:text-gray-400 text-center p-2">
              You do not take part in this channel: its history is shown as it is, not followed live, and you cannot write here.
            </p>
          ) : (
          <form onSubmit={handleSendMessage} className="flex space-x-2">
            {channel && (
              <ClassificationPicker
                channel={channel.classification}
                compartments={compartments}
                value={release ?? channel.classification}
                onChange={(c) => setRelease(sameClassification(c, channel.classification) ? null : c)}
              />
            )}
            <div className="relative flex-1 flex">
              {suggestions.length > 0 && (
                <ul
                  role="listbox"
                  className="absolute bottom-full left-0 mb-1 w-64 max-h-60 overflow-y-auto z-10 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600 rounded-md shadow-lg py-1"
                >
                  {suggestions.map((suggestion, idx) => {
                    const [label, detail, description] = suggestion.kind === 'command'
                      ? [`/${suggestion.command.name}`, suggestion.command.usage ?? '', suggestion.command.description]
                      : [suggestion.member.name, `@${suggestion.member.handle}`, null];
                    return (
                      <li
                        key={`${suggestion.kind}:${label}`}
                        role="option"
                        aria-selected={idx === suggestionIndex}
                        onMouseDown={(e) => { e.preventDefault(); selectSuggestion(suggestion); }}
                        onMouseEnter={() => setSuggestionIndex(idx)}
                        className={cn(
                          "px-3 py-1.5 cursor-pointer",
                          idx === suggestionIndex
                            ? "bg-blue-600 text-white"
                            : "text-gray-800 dark:text-gray-200"
                        )}
                      >
                        <div className="flex items-baseline justify-between gap-2">
                          <span className={cn("text-sm truncate", suggestion.kind === 'command' && "font-mono")}>{label}</span>
                          <span className={cn("text-xs font-mono", idx === suggestionIndex ? "text-blue-100" : "text-gray-500 dark:text-gray-400")}>
                            {detail}
                          </span>
                        </div>
                        {description && (
                          <div className={cn("text-xs truncate", idx === suggestionIndex ? "text-blue-100" : "text-gray-500 dark:text-gray-400")}>
                            {description}
                          </div>
                        )}
                      </li>
                    );
                  })}
                </ul>
              )}
              <input
                ref={inputRef}
                type="text"
                value={inputText}
                onChange={updateInput}
                onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? inputText.length)}
                onKeyDown={handleInputKeyDown}
                onBlur={() => setDismissedMention(suggestionStart)}
                onFocus={() => setDismissedMention(null)}
                placeholder={answersOnlyWhenMentioned(channel?.responder_policy)
                  ? "Agents answer only when @-mentioned... (/ for commands)"
                  : "Type your message... (@ to mention, / for commands)"}
                // Read-only rather than disabled while sending: a disabled
                // field loses the focus, and the user would have to click
                // back into it for the next message.
                readOnly={isSending}
                aria-busy={isSending}
                role="combobox"
                aria-expanded={suggestions.length > 0}
                aria-autocomplete="list"
                className="flex-1 p-2 border border-gray-300 dark:border-gray-600 rounded-md bg-transparent dark:text-white focus:outline-hidden focus:ring-2 focus:ring-blue-500"
              />
            </div>
            <button
              type="submit"
              // Enabled while an agent answers too (issue 202): the runtime
              // takes a message at any time, and an agent in a turn hears it
              // once the turn ends.
              disabled={isSending || !inputText.trim()}
              className="p-2 bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white rounded-md transition-colors"
            >
              <Send size={20} />
            </button>
            {isGenerating && (
              <button
                type="button"
                onClick={handleStopGeneration}
                className="p-2 bg-red-600 hover:bg-red-700 text-white rounded-md transition-colors"
                title="Stop generation"
              >
                <Square size={20} fill="currentColor" />
              </button>
            )}
          </form>
          )}
        </div>
      </div>

      {isAddingParticipant && channel && (
        <AddParticipantDialog
          sessionId={sessionId}
          channel={channel}
          onAdded={(updated) => { setChannel(updated); setIsAddingParticipant(false); }}
          onClose={() => setIsAddingParticipant(false)}
        />
      )}
      {reclassifying && channel && (reclassifying.kind === 'channel' ? (
        <ClassificationDialog
          title={`Classification of ${channel.name}`}
          current={channel.classification}
          levels={CONFIDENTIALITY_LEVELS}
          compartments={compartments}
          releaseWarning="This releases the channel: everything in it - its history, and what its agents know - becomes readable by the wider circle. What was read under the old classification is not retracted by raising it again. Answers the agents are writing right now are discarded."
          onSubmit={async (c) => setChannel(await api.reclassifyChannel(sessionId, channelId, c))}
          onClose={() => setReclassifying(null)}
        />
      ) : reclassifying.message.classification && (
        <ClassificationDialog
          title="Classification of the message"
          current={reclassifying.message.classification}
          levels={levelsUpTo(channel.classification.level)}
          compartments={compartments}
          releaseWarning="This releases the message: it becomes readable by the wider circle."
          onSubmit={(c) => api.reclassifyMessage(sessionId, channelId, reclassifying.message.id!, c)}
          onClose={() => setReclassifying(null)}
        />
      ))}
      {contextOf && contextOf.channelId === channelId ? (
        <ContextPanel
          key={contextOf.message.id}
          sessionId={sessionId}
          channelId={channelId}
          message={contextOf.message}
          members={members}
          onClose={() => setContextOf(null)}
        />
      ) : (
      <div className="w-64 border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 overflow-y-auto hidden md:block">
        <div className="p-4">
          <div className="flex items-center gap-1 mb-4 text-gray-500 dark:text-gray-400" role="tablist">
            <button
              role="tab"
              aria-selected={sideTab === 'members'}
              onClick={() => setSideTab('members')}
              className={cn("flex items-center gap-1.5 px-2 py-1 rounded-md", sideTab === 'members' && "bg-gray-100 dark:bg-gray-800 text-gray-800 dark:text-gray-100")}
            >
              <Users size={16} />
              <h3 className="font-semibold uppercase text-xs tracking-wider">Members</h3>
            </button>
            <button
              role="tab"
              aria-selected={sideTab === 'notes'}
              onClick={() => setSideTab('notes')}
              className={cn("flex items-center gap-1.5 px-2 py-1 rounded-md", sideTab === 'notes' && "bg-gray-100 dark:bg-gray-800 text-gray-800 dark:text-gray-100")}
            >
              <NotebookPen size={16} />
              <h3 className="font-semibold uppercase text-xs tracking-wider">Notes</h3>
            </button>
          </div>
          {sideTab === 'notes' ? <NotesList notes={notes} /> : (<>
          {channel && (
            <div
              className="flex items-center gap-2 mb-4 text-xs text-gray-500 dark:text-gray-400"
              title={responderPolicyInfo(channel.responder_policy).description}
            >
              <Scale size={14} />
              <span>Moderator:</span>
              <span className="text-gray-700 dark:text-gray-300">{responderPolicyInfo(channel.responder_policy).label}</span>
            </div>
          )}
          {channel && (
            <div
              className="flex items-center gap-2 mb-4 -mt-2 text-xs text-gray-500 dark:text-gray-400"
              title={classificationDescription(channel.classification)}
            >
              <Shield size={14} />
              <span>Classification:</span>
              <span className="text-gray-700 dark:text-gray-300">{classificationLabel(channel.classification)}</span>
              {channel.can_change && (
                <button
                  onClick={() => setReclassifying({kind: 'channel'})}
                  className="p-0.5 opacity-60 hover:opacity-100 focus:opacity-100"
                  title="Change the classification"
                  aria-label="Change the classification of the channel"
                >
                  <Pencil size={12} />
                </button>
              )}
            </div>
          )}
          <div className="space-y-2">
            {members.map(member => (
              <div key={actorKey(member.id)} className="group flex items-center space-x-2 p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors">
                <div className="w-2 h-2 rounded-full bg-green-500" />
                <span className="text-sm text-gray-700 dark:text-gray-300 truncate">{member.name}</span>
                {member.handle && <span className="text-xs font-mono text-gray-400 dark:text-gray-500 truncate">@{member.handle}</span>}
                {/* The approver belongs to every channel and cannot be removed. */}
                {channel?.can_change && member.kind !== 'approver' && (
                  <button
                    onClick={() => void removeParticipant(member)}
                    className="ml-auto p-1 rounded-md text-gray-400 opacity-0 group-hover:opacity-100 focus:opacity-100 hover:text-red-500 hover:bg-gray-200 dark:hover:bg-gray-700"
                    title={`Remove ${member.name} from the channel`}
                    aria-label={`Remove ${member.name} from the channel`}
                  >
                    <UserMinus size={14} />
                  </button>
                )}
              </div>
            ))}
          </div>
          {channel?.can_change && (
            <button
              onClick={() => setIsAddingParticipant(true)}
              className="mt-3 flex items-center gap-2 px-2 py-1 rounded-md text-xs text-gray-500 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-800 hover:text-gray-800 dark:hover:text-gray-100"
            >
              <UserPlus size={14} />
              <span>Add participant</span>
            </button>
          )}
          </>)}
        </div>
      </div>
      )}
    </div>
  );
};
