// One tool call as the chat shows it - its tool, who sees it, its status, and its content as its
// presentation says (issue 165). The frame is the same for every call; what is inside it is the
// handler's of the call's presentation, the default one for every call that declares none. The
// frame also shows the questions a call asked and how they were answered, where its result keeps
// their record.

import React from 'react';
import {CheckCircle2, EyeOff, MessageCircleQuestion, Terminal, Wrench, XCircle} from 'lucide-react';
import type {Answers, QuestionResponse, ToolCall, ToolPresentation, ToolResult, ToolResultStatus, ToolVisibility} from '../types';
import {formatArguments, formatTimestamp} from '../messages';

const TOOL_STATUS_LABELS: Record<Exclude<ToolResultStatus, 'ok'>, string> = {
  denied: 'Denied',
  forbidden: 'Not permitted',
  timed_out: 'Timed out',
  declined: 'Declined',
  invalid_arguments: 'Invalid arguments',
  failed: 'Failed',
  not_executed: 'Not executed',
  interrupted: 'Interrupted',
  unknown_tool: 'Unknown tool',
  withheld: 'Withheld',
};

/**
 * What of a call only the agent's owner sees (issues 54, 133), or `null`
 * if the channel sees all of it. The channel shows such a call without
 * arguments and result, or not at all; the owner sees it in full in the
 * context panel.
 */
function visibilityLabel(labels: ToolVisibility | undefined): string | null {
  if (!labels || (labels.call === 'channel' && labels.content === 'channel')) return null;
  return labels.call === 'channel' ? 'Arguments and result: owner only' : 'Call: owner only';
}

/** How long a tool ran, e.g. `240 ms`, `3.1 s` or `2 min 5 s`; `null` if it did not run. */
function toolDuration(result: ToolResult): string | null {
  if (!result.started_at || !result.finished_at) return null;
  const ms = Date.parse(result.finished_at) - Date.parse(result.started_at);
  if (Number.isNaN(ms) || ms < 0) return null;
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.floor(ms / 60_000)} min ${Math.round((ms % 60_000) / 1000)} s`;
}

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

/** A response as the record shows it. */
function responseText(response: QuestionResponse | undefined): string {
  switch (response?.kind) {
    case 'text': return response.text;
    case 'chosen': return [...(response.chosen ?? []), ...(response.other ? [response.other] : [])].join(', ');
    default: return 'declined';
  }
}

/** The record of the questions a call asked and of their answers (issue 165). */
const QuestionsRecord: React.FC<{answers: Answers}> = ({answers}) => {
  const declined = answers.responses.every(r => r.kind === 'declined');
  return (
    <div className="mt-2 pt-2 border-t border-gray-200 dark:border-gray-700 space-y-1">
      <div className="text-[11px] uppercase opacity-50 flex items-center gap-1">
        <MessageCircleQuestion size={11} />
        {declined ? `Declined by ${answers.answered_by_name}` : `Answered by ${answers.answered_by_name}`}
      </div>
      {answers.questions.questions.map((question, index) => (
        <div key={index} className="text-xs">
          <span className="opacity-70">{question.question}</span>{' '}
          <span className={answers.responses[index]?.kind === 'declined' ? 'italic opacity-60' : 'font-medium'}>
            {responseText(answers.responses[index])}
          </span>
        </div>
      ))}
    </div>
  );
};

/** What a presentation's handler is given: the call, and its result once it has one. */
export interface ToolCallContentProps {
  call: ToolCall;
  result?: ToolResult;
}

/** The default presentation: the call's arguments and its result, as they are. */
const DefaultToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => (
  <>
    <div className="flex items-start gap-2 mb-1">
      <Terminal size={12} className="mt-0.5 opacity-50" />
      {call.arguments === null ? (
        <span className="text-[11px] italic opacity-60">Arguments not shown</span>
      ) : (
        <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>
      )}
    </div>
    {result && result.content === '' && (result.visibility?.content ?? 'channel') !== 'channel' ? (
      <div className="mt-2 pt-2 border-t border-gray-200 dark:border-gray-700 text-[11px] italic opacity-60">
        Result not shown
      </div>
    ) : result && (
      <div className="mt-2 pt-2 border-t border-gray-200 dark:border-gray-700">
        <div className="text-[11px] uppercase opacity-50 mb-1">Result</div>
        <pre className="text-[11px] font-mono whitespace-pre-wrap break-all bg-white dark:bg-gray-950 p-1.5 rounded-sm border border-gray-100 dark:border-gray-900">
          {toolResultText(result)}
        </pre>
      </div>
    )}
  </>
);

/** The handler of each presentation; one the server knows and this client does not falls back to the default. */
const PRESENTATIONS: Record<ToolPresentation, React.FC<ToolCallContentProps>> = {
  default: DefaultToolCallContent,
};

interface ToolCallViewProps {
  call: ToolCall;
  /** Its result, once it has one. */
  result?: ToolResult;
  /** Its caller was removed from the channel before it ended. */
  abandoned?: boolean;
}

export const ToolCallView: React.FC<ToolCallViewProps> = ({call, result, abandoned}) => {
  const Content = PRESENTATIONS[call.presentation ?? 'default'] ?? DefaultToolCallContent;
  return (
    <div className="bg-gray-50 dark:bg-gray-800 rounded-sm border border-gray-200 dark:border-gray-600 overflow-hidden">
      <div className="bg-gray-100 dark:bg-gray-900 px-2 py-1 flex items-center justify-between border-b border-gray-200 dark:border-gray-600">
        <div className="flex items-center gap-2 text-xs font-mono">
          <Wrench size={12} className="text-blue-500" />
          <span>{call.function_name}</span>
          {visibilityLabel(call.visibility) && (
            <span className="flex items-center gap-1 font-sans text-[11px] opacity-60" title="Who sees this call">
              <EyeOff size={11} />
              {visibilityLabel(call.visibility)}
            </span>
          )}
        </div>
        <div className="flex items-center gap-2">
        {result && toolDuration(result) && (
          <span
            className="text-[11px] font-mono opacity-60"
            title={result.started_at ? `Ran from ${formatTimestamp(result.started_at)?.full}` : undefined}
          >
            {toolDuration(result)}
          </span>
        )}
        {result && result.status && result.status !== 'ok' ? (
          <div className="flex items-center gap-1 text-amber-600 dark:text-amber-400">
            <XCircle size={12} />
            <span className="text-[11px] uppercase">{TOOL_STATUS_LABELS[result.status]}</span>
          </div>
        ) : result ? (
          <CheckCircle2 size={12} className="text-green-500" />
        ) : abandoned ? (
          <div className="flex items-center gap-1 opacity-60" title="Its caller was removed from the channel">
            <XCircle size={12} />
            <span className="text-[11px] uppercase">Abandoned</span>
          </div>
        ) : (
          <div className="flex items-center gap-1">
            <div className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse" />
            <span className="text-[11px] text-blue-500 uppercase">Executing</span>
          </div>
        )}
        </div>
      </div>
      <div className="p-2">
        <Content call={call} result={result} />
        {result?.answers && <QuestionsRecord answers={result.answers} />}
      </div>
    </div>
  );
};
