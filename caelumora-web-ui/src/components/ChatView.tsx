import React, {useEffect, useRef, useState} from 'react';
import {
  AlertTriangle,
  CheckCircle2,
  Send,
  Square,
  Terminal,
  Users,
  Wrench,
  XCircle
} from 'lucide-react';
import type {
  ApprovalRequest,
  ChannelMember,
  ChannelResponse,
  ChannelSnapshot,
  Envelope,
  Message,
  PlanStepEvent,
  ToolResult,
  ToolResultStatus
} from '../types';
import {api} from '../api';
import {reportError} from '../errors';
import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';
import {MarkdownMessage} from './MarkdownMessage';
import {mentionedHandles, mentionQueryAt} from '../mention';
import {loadDraft, saveDraft} from '../session';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

const TOOL_STATUS_LABELS: Record<Exclude<ToolResultStatus, 'ok'>, string> = {
  denied: 'Denied',
  forbidden: 'Not permitted',
  timed_out: 'Timed out',
  invalid_arguments: 'Invalid arguments',
  failed: 'Failed',
  unknown_tool: 'Unknown tool',
};

/** The text to show for a tool result: the message of an unsuccessful one, else the content. */
function toolResultText(result: ToolResult): string {
  if (!result.status || result.status === 'ok') return result.content;
  try {
    const parsed = JSON.parse(result.content);
    if (typeof parsed?.message === 'string') return parsed.message;
  } catch {
    // Not the JSON object the server writes for unsuccessful results.
  }
  return result.content;
}

function formatArguments(args: unknown): string {
  return typeof args === 'string' ? args : JSON.stringify(args);
}

type MentionableMember = ChannelMember & {handle: string};

interface ChatViewProps {
  sessionId: string;
  channelId: string;
}

export const ChatView: React.FC<ChatViewProps> = ({ sessionId, channelId }) => {
  const [messages, setMessages] = useState<Message[]>([]);
  const [channel, setChannel] = useState<ChannelResponse | null>(null);
  // Restored from session storage, so a forced re-login does not lose it.
  const [inputText, setInputText] = useState(() => loadDraft(channelId));
  const [isSending, setIsSending] = useState(false);
  const [isGenerating, setIsGenerating] = useState(false);
  const [streamingMessage, setStreamingMessage] = useState<string | null>(null);
  const [streamingSender, setStreamingSender] = useState<string | null>(null);
  const [pendingApprovals, setPendingApprovals] = useState<ApprovalRequest[]>([]);
  const [currentStep, setCurrentStep] = useState<PlanStepEvent | null>(null);
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

  // The server returns members in no particular order.
  const members: ChannelMember[] = [...(channel?.members ?? [])].sort((l, r) => l.name.localeCompare(r.name));
  const mentionQuery = mentionQueryAt(inputText, caret);
  const suggestions: MentionableMember[] = mentionQuery && mentionQuery.start !== dismissedMention
    ? members.filter((m): m is MentionableMember => {
        const query = mentionQuery.query.toLowerCase();
        return !!m.handle && (m.handle.startsWith(query) || m.name.toLowerCase().includes(query));
      })
    : [];

  useEffect(() => {
    let stopped = false;
    let current: EventSource | null = null;

    const handleSnapshot = (event: MessageEvent) => {
      try {
        const snapshot: ChannelSnapshot = JSON.parse(event.data);
        setMessages(snapshot.messages);
        setChannel(snapshot.channel);
        setHasMore(snapshot.has_more);
        setStreamingMessage(null);
        setStreamingSender(null);
        setIsGenerating(false);
      } catch (e) {
        reportError('Failed to read the channel state', e);
      }
    };

    const handleMessage = (event: MessageEvent) => {
      try {
        const envelope: Envelope = JSON.parse(event.data);
        const actorEvent = envelope.event;
        if (actorEvent.ResponseStarted) {
          setIsGenerating(true);
          setStreamingMessage('');
          setStreamingSender(envelope.context.sender_name || null);
        } else if (actorEvent.PartialMessageGenerated) {
          setIsGenerating(true);
          setStreamingMessage(prev => (prev || '') + (actorEvent.PartialMessageGenerated!.content || ''));
          setStreamingSender(envelope.context.sender_name || null);
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
          setStreamingMessage(null);
          setStreamingSender(null);
        } else if (actorEvent.ApprovalRequested) {
          setPendingApprovals(prev => [...prev, actorEvent.ApprovalRequested!]);
        } else if (actorEvent.ApprovalGranted) {
          const res = actorEvent.ApprovalGranted;
          setPendingApprovals(prev => prev.filter(req => req.id !== res!.id));
        } else if (actorEvent.ApprovalDenied) {
          const res = actorEvent.ApprovalDenied;
          setPendingApprovals(prev => prev.filter(req => req.id !== res!.id));
        } else if (actorEvent.FullResponseGenerated) {
          //setMessages(prev => [...prev, ...actorEvent.FullResponseGenerated!]);
          setStreamingMessage(null);
          setStreamingSender(null);
          setIsGenerating(false);
          setCurrentStep(null);
        } else if (actorEvent.TurnFailed) {
          // Shown in the chat, but not part of the history: gone on reload.
          const failed: Message = {
            role: 'System',
            content: actorEvent.TurnFailed.message,
            sender_name: envelope.context.sender_name,
            turnFailed: true,
            span_id: envelope.context.span_id,
          };
          setMessages(prev => [...prev, failed]);
          setStreamingMessage(null);
          setStreamingSender(null);
          setIsGenerating(false);
          setCurrentStep(null);
        } else if (actorEvent.PlanStepStarted) {
          setCurrentStep(actorEvent.PlanStepStarted);
        } else if (actorEvent.PlanStepFinished) {
          setCurrentStep(prev =>
            prev && prev.step_index === actorEvent.PlanStepFinished!.step_index ? null : prev
          );
        }
      } catch (e) {
        console.error('Failed to parse SSE event', e);
      }
    };

    // The stream itself delivers the state it starts from, so there is no
    // window between loading the history and subscribing in which a message
    // could be missed (issue 34).
    const connect = () => {
      const eventSource = api.getEventSource(sessionId, channelId, {snapshot: true});
      current = eventSource;
      eventSource.addEventListener('snapshot', handleSnapshot);
      eventSource.onmessage = handleMessage;

      // The server ends a stream shortly before the session token that
      // authenticated it expires (issue 33). Reconnecting at once keeps the
      // gap invisible: the new request carries the cookie the keep-alive has
      // renewed, and its snapshot restores what happened in between. Waiting
      // for the browser's own retry would freeze the output for seconds.
      eventSource.addEventListener('expired', () => {
        eventSource.close();
        if (!stopped) connect();
      });

      eventSource.onerror = (e) => {
        // A connection that broke unexpectedly: the browser retries by
        // itself, so leave it alone as long as it is doing so.
        if (eventSource.readyState === EventSource.CONNECTING) return;
        console.error('SSE Error', e);
        eventSource.close();
        // EventSource does not expose the status code; if the session has
        // expired, this request's 401 brings up the login page.
        void api.me();
      };
    };

    connect();
    return () => {
      stopped = true;
      current?.close();
    };
  }, [sessionId, channelId]);


  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages, streamingMessage, isGenerating]);

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

    try {
      await api.sendMessage(sessionId, channelId, text);
    } catch (error) {
      reportError('Failed to send message', error);
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
      selectMention(suggestions[Math.min(suggestionIndex, suggestions.length - 1)]);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      setDismissedMention(mentionQuery?.start ?? null);
    }
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
      setPendingApprovals(prev => prev.filter(a => a.id !== approval.id));
    } catch (e) {
      reportError('Failed to respond to approval', e);
    }
  };

  return (
    <div className="flex-1 flex overflow-hidden bg-gray-50 dark:bg-gray-800">
      <div className="flex-1 flex flex-col min-w-0">
        {currentStep && (
          <div className="px-4 py-2 bg-blue-50 dark:bg-blue-900/20 border-b border-blue-200 dark:border-blue-900/50 flex items-center gap-2">
            <div className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse shrink-0" />
            <span className="text-xs font-semibold uppercase tracking-wider text-blue-700 dark:text-blue-300 shrink-0">
              Step {currentStep.step_index + 1} of {currentStep.total_steps}
            </span>
            <span className="text-xs text-blue-700 dark:text-blue-300 truncate">
              {currentStep.description}
            </span>
          </div>
        )}
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
          {messages.reduce((acc: Message[], msg) => {
            // Logic to merge tool calls and results
            if (msg.tool_results && msg.tool_results.length > 0) {
              const lastMsg = acc[acc.length - 1];
              if (lastMsg && lastMsg.tool_calls && lastMsg.tool_calls.some(tc => msg.tool_results?.some(tr => tr.id === tc.id))) {
                // Merge tool result into previous message that had the tool call
                const newLastMsg = { 
                  ...lastMsg, 
                  tool_results: [...(lastMsg.tool_results || []), ...msg.tool_results] 
                };
                return [...acc.slice(0, -1), newLastMsg];
              }
            }
            return [...acc, msg];
          }, []).map((msg, idx) => msg.turnFailed ? (
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
                <div className="mt-1 text-[10px] font-mono opacity-60">span_id {msg.span_id}</div>
              )}
            </div>
          ) : (
            <div
              key={idx}
              className={cn(
                "max-w-[80%] p-3 rounded-lg shadow-xs",
                msg.role === 'User' 
                  ? "ml-auto bg-blue-600 text-white rounded-br-none" 
                  : "bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 rounded-bl-none border border-gray-200 dark:border-gray-600"
              )}
            >
              <div className="text-xs font-semibold mb-1 opacity-70 uppercase tracking-wider flex items-center gap-2">
                {msg.sender_name || msg.role}
              </div>
              
              {msg.content && (
                <MarkdownMessage
                  content={msg.content}
                  invert={msg.role === 'User'}
                  mentions={mentionedHandles(msg, members)}
                />
              )}

              {msg.tool_calls && msg.tool_calls.length > 0 && (
                <div className="mt-2 space-y-2">
                  {msg.tool_calls.map((call, cidx) => {
                    const result = msg.tool_results?.find(r => r.id === call.id);
                    return (
                      <div key={cidx} className="bg-gray-50 dark:bg-gray-800 rounded-sm border border-gray-200 dark:border-gray-600 overflow-hidden">
                        <div className="bg-gray-100 dark:bg-gray-900 px-2 py-1 flex items-center justify-between border-b border-gray-200 dark:border-gray-600">
                          <div className="flex items-center gap-2 text-xs font-mono">
                            <Wrench size={12} className="text-blue-500" />
                            <span>{call.function_name}</span>
                          </div>
                          {result && result.status && result.status !== 'ok' ? (
                            <div className="flex items-center gap-1 text-amber-600 dark:text-amber-400">
                              <XCircle size={12} />
                              <span className="text-[10px] uppercase">{TOOL_STATUS_LABELS[result.status]}</span>
                            </div>
                          ) : result ? (
                            <CheckCircle2 size={12} className="text-green-500" />
                          ) : (
                            <div className="flex items-center gap-1">
                               <div className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse" />
                               <span className="text-[10px] text-blue-500 uppercase">Executing</span>
                            </div>
                          )}
                        </div>
                        <div className="p-2">
                           <div className="flex items-start gap-2 mb-1">
                             <Terminal size={12} className="mt-0.5 opacity-50" />
                             <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>
                           </div>
                           {result && (
                             <div className="mt-2 pt-2 border-t border-gray-200 dark:border-gray-700">
                               <div className="text-[10px] uppercase opacity-50 mb-1">Result</div>
                               <pre className="text-[11px] font-mono whitespace-pre-wrap break-all bg-white dark:bg-gray-950 p-1.5 rounded-sm border border-gray-100 dark:border-gray-900">
                                 {toolResultText(result)}
                               </pre>
                             </div>
                           )}
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          ))}
          {(isGenerating || streamingMessage) && (
            <div className="max-w-[80%] p-3 rounded-lg shadow-xs bg-white dark:bg-gray-700 text-gray-900 dark:text-gray-100 rounded-bl-none border border-gray-200 dark:border-gray-600">
              <div className="text-xs font-semibold mb-1 opacity-70 uppercase tracking-wider flex items-center gap-2">
                <span>{streamingSender || 'Assistant'}</span>
                {isGenerating ? (
                  <span className="inline-flex items-center gap-1.5 normal-case font-normal text-blue-500 dark:text-blue-400">
                    <span className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse" />
                    generating…
                  </span>
                ) : (
                  <span className="normal-case font-normal text-red-500 dark:text-red-400">stopped</span>
                )}
              </div>
              {streamingMessage && <MarkdownMessage content={streamingMessage} />}
            </div>
          )}
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
                   <div className="text-[10px] text-gray-400">ID: {approval.id}</div>
                </div>
                <pre className="text-xs font-mono bg-gray-50 dark:bg-gray-900 p-2 rounded-sm mb-4 overflow-x-auto">
                  {formatArguments(approval.tool_call.arguments)}
                </pre>
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
              </div>
            ))}
          </div>
        )}

        <div className="p-4 bg-white dark:bg-gray-900 border-t border-gray-200 dark:border-gray-700">
          <form onSubmit={handleSendMessage} className="flex space-x-2">
            <div className="relative flex-1 flex">
              {suggestions.length > 0 && (
                <ul
                  role="listbox"
                  className="absolute bottom-full left-0 mb-1 w-64 max-h-60 overflow-y-auto z-10 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600 rounded-md shadow-lg py-1"
                >
                  {suggestions.map((member, idx) => (
                    <li
                      key={member.handle + idx}
                      role="option"
                      aria-selected={idx === suggestionIndex}
                      onMouseDown={(e) => { e.preventDefault(); selectMention(member); }}
                      onMouseEnter={() => setSuggestionIndex(idx)}
                      className={cn(
                        "px-3 py-1.5 cursor-pointer flex items-baseline justify-between gap-2",
                        idx === suggestionIndex
                          ? "bg-blue-600 text-white"
                          : "text-gray-800 dark:text-gray-200"
                      )}
                    >
                      <span className="text-sm truncate">{member.name}</span>
                      <span className={cn("text-xs font-mono", idx === suggestionIndex ? "text-blue-100" : "text-gray-500 dark:text-gray-400")}>
                        @{member.handle}
                      </span>
                    </li>
                  ))}
                </ul>
              )}
              <input
                ref={inputRef}
                type="text"
                value={inputText}
                onChange={updateInput}
                onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? inputText.length)}
                onKeyDown={handleInputKeyDown}
                onBlur={() => setDismissedMention(mentionQuery?.start ?? null)}
                onFocus={() => setDismissedMention(null)}
                placeholder="Type your message... (@ to mention)"
                disabled={isSending}
                role="combobox"
                aria-expanded={suggestions.length > 0}
                aria-autocomplete="list"
                className="flex-1 p-2 border border-gray-300 dark:border-gray-600 rounded-md bg-transparent dark:text-white focus:outline-hidden focus:ring-2 focus:ring-blue-500"
              />
            </div>
            <button
              type="submit"
              disabled={isSending || isGenerating || !inputText.trim()}
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
        </div>
      </div>

      <div className="w-64 border-l border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900 overflow-y-auto hidden md:block">
        <div className="p-4">
          <div className="flex items-center space-x-2 mb-4 text-gray-500 dark:text-gray-400">
            <Users size={18} />
            <h3 className="font-semibold uppercase text-xs tracking-wider">Channel Members</h3>
          </div>
          <div className="space-y-2">
            {members.map((member, idx) => (
              <div key={idx} className="flex items-center space-x-2 p-2 rounded-md hover:bg-gray-100 dark:hover:bg-gray-800 transition-colors">
                <div className="w-2 h-2 rounded-full bg-green-500" />
                <span className="text-sm text-gray-700 dark:text-gray-300">{member.name}</span>
                {member.handle && <span className="text-xs font-mono text-gray-400 dark:text-gray-500">@{member.handle}</span>}
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
};
