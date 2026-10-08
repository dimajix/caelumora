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
          <span className="opacity-70 whitespace-pre-wrap">{question.question}</span>{' '}
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

/** A call's arguments as an object, whether they came as one or as its JSON text; `null` if neither. */
function argumentsObject(args: unknown): Record<string, unknown> | null {
  let value = args;
  if (typeof value === 'string') {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

/** What a shell tool answers of a command (issue 251). */
interface CommandResult {
  id: string;
  status: 'running' | 'completed' | 'killed' | 'failed';
  output: string;
  truncated: boolean;
  exit_code?: number;
}

/** The result of a successful call of a shell tool, or `null` for any other. */
function commandResult(result: ToolResult): CommandResult | null {
  if (result.status && result.status !== 'ok') return null;
  try {
    const parsed = JSON.parse(result.content);
    return typeof parsed?.id === 'string' && typeof parsed?.status === 'string' && typeof parsed?.output === 'string' ? parsed : null;
  } catch {
    return null;
  }
}

/** Where a command stands, as its result says. */
function commandStatus(command: CommandResult): string {
  switch (command.status) {
    case 'running': return `Still running as ${command.id}`;
    case 'killed': return 'Killed';
    case 'failed': return 'Failed';
    default: return command.exit_code === undefined ? 'Completed' : `Exit code ${command.exit_code}`;
  }
}

/**
 * A shell command (issue 251): the command as a line of code, what it printed as a terminal shows it,
 * and where it stands. A call that follows or ends a command started before names it by its id.
 */
const ShellToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => {
  const args = argumentsObject(call.arguments);
  const command = typeof args?.command === 'string' ? args.command : null;
  const followed = typeof args?.id === 'string' ? args.id : null;
  const answered = result && commandResult(result);
  const failed = answered && (answered.status === 'failed' || (answered.exit_code ?? 0) !== 0);
  return (
    <>
      {call.arguments === null ? (
        <span className="text-[11px] italic opacity-60">Command not shown</span>
      ) : command !== null ? (
        <pre className="text-[11px] font-mono whitespace-pre-wrap break-all"><span className="select-none opacity-50">$ </span>{command}</pre>
      ) : followed !== null ? (
        <div className="text-[11px] opacity-70">
          {call.function_name === 'kill_command' ? 'Ends' : 'Follows'} command <code className="font-mono">{followed}</code>
        </div>
      ) : (
        <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>
      )}
      {result && result.content === '' && (result.visibility?.content ?? 'channel') !== 'channel' ? (
        <div className="mt-2 text-[11px] italic opacity-60">Output not shown</div>
      ) : answered ? (
        <div className="mt-2">
          {(answered.output !== '' || answered.truncated) && (
            <pre className="text-[11px] font-mono whitespace-pre-wrap break-all max-h-80 overflow-y-auto bg-gray-900 text-gray-100 p-1.5 rounded-sm">
              {answered.truncated && <span className="block italic opacity-60">… earlier output dropped</span>}
              {answered.output}
            </pre>
          )}
          <div className={`mt-1 text-[11px] ${failed ? 'text-amber-600 dark:text-amber-400' : 'opacity-60'}`}>
            {commandStatus(answered)}
          </div>
        </div>
      ) : result && (
        <pre className="mt-2 text-[11px] font-mono whitespace-pre-wrap break-all bg-white dark:bg-gray-950 p-1.5 rounded-sm border border-gray-100 dark:border-gray-900">
          {toolResultText(result)}
        </pre>
      )}
    </>
  );
};

/** The steps of a plan's arguments, each its description, or `null` if they are no list of steps. */
function planSteps(steps: unknown): string[] | null {
  if (!Array.isArray(steps)) return null;
  const descriptions = steps.map(step => typeof step?.description === 'string' ? step.description as string : null);
  return descriptions.every(d => d !== null) ? descriptions as string[] : null;
}

/**
 * A plan (issue 263): its summary and its steps, numbered - a plan extended is a call of its own,
 * whose steps follow those before. A call that failed shows why.
 */
const PlanToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => {
  const args = argumentsObject(call.arguments);
  const summary = typeof args?.summary === 'string' ? args.summary : null;
  const steps = planSteps(args?.steps);
  const failed = result && result.status && result.status !== 'ok';
  return (
    <>
      {call.arguments === null ? (
        <span className="text-[11px] italic opacity-60">Plan not shown</span>
      ) : summary !== null && steps !== null ? (
        <div className="text-xs">
          <div className="font-medium">{summary}</div>
          <ol className="mt-1 ml-5 list-decimal space-y-0.5">
            {steps.map((step, i) => <li key={i}>{step}</li>)}
          </ol>
        </div>
      ) : (
        <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>
      )}
      {failed && (
        <pre className="mt-2 text-[11px] font-mono whitespace-pre-wrap break-all bg-white dark:bg-gray-950 p-1.5 rounded-sm border border-gray-100 dark:border-gray-900">
          {toolResultText(result)}
        </pre>
      )}
    </>
  );
};

/** The handler of each presentation; one the server knows and this client does not falls back to the default. */
const PRESENTATIONS: Record<ToolPresentation, React.FC<ToolCallContentProps>> = {
  default: DefaultToolCallContent,
  shell: ShellToolCallContent,
  plan: PlanToolCallContent,
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
