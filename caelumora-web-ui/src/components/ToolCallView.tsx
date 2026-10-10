// One tool call as the chat shows it - its tool, who sees it, its status, and its content as its
// presentation says (issue 165). The frame is the same for every call; what is inside it is the
// handler's of the call's presentation, the default one for every call that declares none. The
// frame also shows the questions a call asked and how they were answered, where its result keeps
// their record (`AnswersRecord`, issue 268).

import React, {useEffect, useState} from 'react';
import {BookOpen, CheckCircle2, Download, EyeOff, Package, PackageCheck, File, Square, FileDiff, FileDown, FileInput, FilePen, FileText, Folder, FolderOpen, TextSearch, Terminal, Wrench, XCircle} from 'lucide-react';
import type {ArtifactResponse, Question, ToolCall, ToolPresentation, ToolResult, ToolResultStatus, ToolVisibility} from '../types';
import {api, type ChannelArtifacts, type WorkspaceFiles} from '../api';
import type {Page} from '../page';
import {ApiRequestError, reportError} from '../errors';
import {commandResult, formatArguments, formatSize, formatTimestamp, ownerOnly, type CommandResult} from '../messages';
import {AnswersRecord, ChoiceRows, QuestionHeading} from './QuestionView';
import {MarkdownMessage} from './MarkdownMessage';

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
 * What of a call only the agent's owner sees (issues 54, 133, 281), or `null`
 * if the channel sees all of it. The channel shows such a call without
 * what is the owner's, or not at all; the owner sees it in full in the
 * context panel.
 */
function visibilityLabel(labels: ToolVisibility | undefined): string | null {
  const owners = ownerOnly(labels);
  return owners && `${owners.charAt(0).toUpperCase()}${owners.slice(1)}: owner only`;
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

/** What a presentation's handler is given: the call, its result once it has one, and the files of the channel's workspace where it is shown in the chat. */
export interface ToolCallContentProps {
  call: ToolCall;
  result?: ToolResult;
  /** The files of the channel's workspace as the user may use them; absent outside the chat. */
  files?: WorkspaceFiles;
  /** Changes whenever those files may have changed. */
  filesVersion?: number;
  /** The artifacts of the channel as the user reaches them; absent outside the chat. */
  artifacts?: ChannelArtifacts;
  /** Opens a page in the place of the channel - a skill's; absent outside the chat. */
  onSelectPage?: (page: Page) => void;
  /** Ends a command a call handed back as running (issue 294); absent where the user may not change the workspace's files. */
  onKillCommand?: (commandId: string) => void;
  /** The commands a result in the channel, or the user, ended: none is offered to be ended again. */
  endedCommands?: ReadonlySet<string>;
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
    {resultHidden(result) ? (
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
const ShellToolCallContent: React.FC<ToolCallContentProps> = ({call, result, onKillCommand, endedCommands}) => {
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
      {resultHidden(result) ? (
        <div className="mt-2 text-[11px] italic opacity-60">Output not shown</div>
      ) : answered ? (
        <div className="mt-2">
          {(answered.output !== '' || answered.truncated) && (
            <pre className="text-[11px] font-mono whitespace-pre-wrap break-all max-h-80 overflow-y-auto bg-gray-900 text-gray-100 p-1.5 rounded-sm">
              {answered.truncated && <span className="block italic opacity-60">… earlier output dropped</span>}
              {answered.output}
            </pre>
          )}
          <div className="mt-1 flex items-center gap-1 text-[11px]">
            <span className={failed ? 'text-amber-600 dark:text-amber-400' : 'opacity-60'}>{commandStatus(answered)}</span>
            {answered.status === 'running' && onKillCommand && !endedCommands?.has(answered.id) && (
              <button
                type="button"
                onClick={() => onKillCommand(answered.id)}
                className="p-0.5 rounded-sm text-gray-400 hover:text-red-600 dark:hover:text-red-400 transition-colors"
                title="End this command and what it started"
                aria-label="End this command"
              >
                <Square size={10} fill="currentColor" />
              </button>
            )}
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

/** The fields of a successful result's JSON object, or `null` for any other result. */
function resultObject(result: ToolResult | undefined): Record<string, unknown> | null {
  if (!result || (result.status && result.status !== 'ok')) return null;
  return argumentsObject(result.content);
}

/** Whether `result` was emptied for the channel: it is the owner's (issues 133, 281). */
function resultHidden(result: ToolResult | undefined): boolean {
  return !!result && result.content === '' && (result.visibility?.result ?? 'channel') !== 'channel';
}

/** A field of `fields` if it is a string, else `null`. */
function textOf(fields: Record<string, unknown> | null, key: string): string | null {
  const value = fields?.[key];
  return typeof value === 'string' ? value : null;
}

/** A field of `fields` if it is a number, else `null`. */
function numberOf(fields: Record<string, unknown> | null, key: string): number | null {
  const value = fields?.[key];
  return typeof value === 'number' ? value : null;
}

const TEXT_TONES = {
  plain: 'bg-white dark:bg-gray-950 border-gray-100 dark:border-gray-900',
  removed: 'bg-red-50 dark:bg-red-950/40 border-red-100 dark:border-red-900/60',
  added: 'bg-green-50 dark:bg-green-950/40 border-green-100 dark:border-green-900/60',
};

/**
 * The text of a file, or a piece of it, as it is - its lines kept, a line too long for the chat
 * wrapped at word boundaries, and a long text scrolling within its box.
 */
const FileTextBlock: React.FC<{text: string; tone?: keyof typeof TEXT_TONES; label?: string}> = ({text, tone = 'plain', label}) => (
  <div className="mt-1">
    {label && <div className="text-[11px] uppercase opacity-50 mb-0.5">{label}</div>}
    <pre className={`text-[11px] font-mono whitespace-pre-wrap wrap-break-word max-h-80 overflow-y-auto p-1.5 rounded-sm border ${TEXT_TONES[tone]}`}>
      {text === '' ? <span className="italic opacity-60">empty</span> : text}
    </pre>
  </div>
);

/** The file a call is about: its path, after an icon of what is done to it. */
const FilePath: React.FC<{icon: React.ReactNode; path: string | null; detail?: string | null}> = ({icon, path, detail}) => (
  <div className="flex items-start gap-1.5 text-[11px]">
    <span className="mt-0.5 opacity-60 shrink-0">{icon}</span>
    <span className="min-w-0">
      <code className="font-mono break-all">{path ?? '?'}</code>
      {detail && <span className="opacity-60"> · {detail}</span>}
    </span>
  </div>
);

/** What a call that did not succeed says, and nothing for one that did or has no result yet. */
const FailedResult: React.FC<{result?: ToolResult}> = ({result}) =>
  result && result.status && result.status !== 'ok' ? (
    <pre className="mt-2 text-[11px] font-mono whitespace-pre-wrap wrap-break-word bg-white dark:bg-gray-950 p-1.5 rounded-sm border border-gray-100 dark:border-gray-900">
      {toolResultText(result)}
    </pre>
  ) : null;

/**
 * A file read (issue 251): its path, the lines it gave - all of them, or which of how many - and
 * their text, and where to read on if not all of it fitted.
 */
const FileReadToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => {
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">File not shown</span>;
  const args = argumentsObject(call.arguments);
  const read = resultObject(result);
  const content = textOf(read, 'content');
  const [start, end, total] = [numberOf(read, 'start_line'), numberOf(read, 'end_line'), numberOf(read, 'total_lines')];
  const lines = start !== null && end !== null && total !== null
    ? (start <= 1 && end >= total ? `${total} ${total === 1 ? 'line' : 'lines'}` : `lines ${start}-${end} of ${total}`)
    : null;
  return (
    <>
      <FilePath icon={<FileText size={12} />} path={textOf(read, 'path') ?? textOf(args, 'path')} detail={lines} />
      {resultHidden(result) ? (
        <div className="mt-2 text-[11px] italic opacity-60">Content not shown</div>
      ) : content !== null ? (
        <>
          <FileTextBlock text={content} />
          {textOf(read, 'next') && <div className="mt-1 text-[11px] opacity-60">{textOf(read, 'next')}</div>}
        </>
      ) : (
        <FailedResult result={result} />
      )}
    </>
  );
};

/** A file written: its path, what it is to hold, and how many bytes were written. */
const FileWriteToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => {
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">File not shown</span>;
  const args = argumentsObject(call.arguments);
  const content = textOf(args, 'content');
  const bytes = numberOf(resultObject(result), 'bytes');
  return (
    <>
      <FilePath icon={<FilePen size={12} />} path={textOf(args, 'path')} detail={bytes !== null ? `${bytes} ${bytes === 1 ? 'byte' : 'bytes'} written` : null} />
      {content !== null ? <FileTextBlock text={content} /> : <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>}
      <FailedResult result={result} />
    </>
  );
};

/**
 * A replacement in a file: its path, the text replaced and the text put in its place, once or
 * everywhere, and how many places it replaced.
 */
const FileEditToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => {
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">Change not shown</span>;
  const args = argumentsObject(call.arguments);
  const [oldText, newText] = [textOf(args, 'old_text'), textOf(args, 'new_text')];
  const replaced = numberOf(resultObject(result), 'replacements');
  const how = args?.replace_all === true ? 'every occurrence' : null;
  const detail = [how, replaced !== null ? `${replaced} ${replaced === 1 ? 'replacement' : 'replacements'}` : null].filter(Boolean).join(' · ') || null;
  return (
    <>
      <FilePath icon={<FileDiff size={12} />} path={textOf(args, 'path')} detail={detail} />
      {oldText !== null && newText !== null ? (
        <>
          <FileTextBlock text={oldText} tone="removed" label="Replaced" />
          <FileTextBlock text={newText} tone="added" label="By" />
        </>
      ) : (
        <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>
      )}
      <FailedResult result={result} />
    </>
  );
};

/** A count of things, e.g. `1 match` or `3 matches`. */
function counted(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** A directory of the workspace as a call names it: its path, `.` for the workspace's top. */
function directoryPath(path: string | null): string | null {
  return path === '' ? '.' : path;
}

/** The fields of each object of `list`, or `null` if it is no list of objects. */
function objectsOf(list: unknown): Record<string, unknown>[] | null {
  if (!Array.isArray(list)) return null;
  const objects = list.map(argumentsObject);
  return objects.every(o => o !== null) ? objects as Record<string, unknown>[] : null;
}

/** A box of entries or matches, as a file's text is boxed: a long one scrolls within it. */
const ResultBox: React.FC<{children: React.ReactNode}> = ({children}) => (
  <div className={`mt-1 text-[11px] max-h-80 overflow-y-auto p-1.5 rounded-sm border ${TEXT_TONES.plain}`}>
    {children}
  </div>
);

/**
 * A directory listed (issue 265): its path and how many entries it has, and the entries as the file
 * browser shows them - a directory or a file, its name, a file's size, when it was changed.
 */
const FileListToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => {
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">Directory not shown</span>;
  const args = argumentsObject(call.arguments);
  const listed = resultObject(result);
  const entries = objectsOf(listed?.entries);
  const complete = listed?.complete !== false;
  const detail = entries !== null ? (complete ? counted(entries.length, 'entry', 'entries') : `first ${counted(entries.length, 'entry', 'entries')}`) : null;
  return (
    <>
      <FilePath icon={<FolderOpen size={12} />} path={directoryPath(textOf(listed, 'path') ?? textOf(args, 'path') ?? '')} detail={detail} />
      {resultHidden(result) ? (
        <div className="mt-2 text-[11px] italic opacity-60">Entries not shown</div>
      ) : entries !== null ? (
        <>
          <ResultBox>
            {entries.length === 0 ? <span className="italic opacity-60">empty</span> : (
              <ul className="space-y-0.5">
                {entries.map((entry, i) => {
                  const isDir = entry.is_dir === true;
                  const size = numberOf(entry, 'size');
                  const modified = formatTimestamp(textOf(entry, 'modified') ?? undefined);
                  return (
                    <li key={i} className="flex items-center gap-2">
                      {isDir ? <Folder size={12} className="shrink-0 opacity-50" /> : <File size={12} className="shrink-0 opacity-50" />}
                      <span className="flex-1 min-w-0 font-mono break-all">{textOf(entry, 'name') ?? '?'}{isDir && '/'}</span>
                      {!isDir && size !== null && <span className="shrink-0 opacity-60">{formatSize(size)}</span>}
                      {modified && <span className="shrink-0 opacity-60 hidden lg:inline" title={modified.full}>{modified.short}</span>}
                    </li>
                  );
                })}
              </ul>
            )}
          </ResultBox>
          {!complete && <div className="mt-1 text-[11px] opacity-60">The directory has more entries than one listing gives.</div>}
        </>
      ) : (
        <FailedResult result={result} />
      )}
    </>
  );
};

/** The matches of a search, those in one file together, in the order they were found. */
function matchesByFile(matches: Record<string, unknown>[]): {path: string; lines: {line: number | null; text: string}[]}[] {
  const files: {path: string; lines: {line: number | null; text: string}[]}[] = [];
  for (const match of matches) {
    const path = textOf(match, 'path') ?? '?';
    const line = {line: numberOf(match, 'line'), text: textOf(match, 'text') ?? ''};
    const last = files[files.length - 1];
    if (last?.path === path) last.lines.push(line);
    else files.push({path, lines: [line]});
  }
  return files;
}

/**
 * Files searched (issue 265): the pattern and where it was searched, how many matches in how many
 * files, and the matches by file - each its line number and its line. A search that stopped
 * before its end, and files skipped as no text, say so.
 */
const FileSearchToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => {
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">Search not shown</span>;
  const args = argumentsObject(call.arguments);
  const pattern = textOf(args, 'pattern');
  const found = resultObject(result);
  const matches = objectsOf(found?.matches);
  const files = matches !== null ? matchesByFile(matches) : null;
  const searched = numberOf(found, 'files_searched');
  const skipped = numberOf(found, 'files_skipped') ?? 0;
  const detail = matches !== null && files !== null
    ? [`${counted(matches.length, 'match', 'matches')} in ${counted(files.length, 'file', 'files')}`, searched !== null ? `${counted(searched, 'file', 'files')} searched` : null].filter(Boolean).join(' · ')
    : null;
  return (
    <>
      <FilePath icon={<TextSearch size={12} />} path={directoryPath(textOf(args, 'path') ?? '')} detail={detail} />
      {pattern !== null ? (
        <pre className="mt-1 text-[11px] font-mono whitespace-pre-wrap break-all"><span className="select-none opacity-50">/</span>{pattern}<span className="select-none opacity-50">/</span></pre>
      ) : (
        <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>
      )}
      {resultHidden(result) ? (
        <div className="mt-2 text-[11px] italic opacity-60">Matches not shown</div>
      ) : files !== null ? (
        <>
          {files.length > 0 && (
            <ResultBox>
              {files.map((file, i) => (
                <div key={i} className={i > 0 ? 'mt-1.5' : undefined}>
                  <div className="font-mono font-medium break-all">{file.path}</div>
                  {file.lines.map((match, j) => (
                    <div key={j} className="flex gap-2 font-mono">
                      <span className="shrink-0 min-w-8 text-right select-none opacity-50">{match.line ?? '?'}</span>
                      <span className="min-w-0 whitespace-pre-wrap wrap-break-word">{match.text}</span>
                    </div>
                  ))}
                </div>
              ))}
            </ResultBox>
          )}
          {found?.complete === false && <div className="mt-1 text-[11px] opacity-60">The search stopped before its end, at the most matches or files it gives.</div>}
          {skipped > 0 && <div className="mt-1 text-[11px] opacity-60">{counted(skipped, 'file', 'files')} skipped: no text, or too large.</div>}
        </>
      ) : (
        <FailedResult result={result} />
      )}
    </>
  );
};

/**
 * A file or directory moved or renamed (issue 267): where it lay and where it lies now, and whether
 * it was renamed - moved within its directory - or moved, as its result says.
 */
const FileMoveToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => {
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">Move not shown</span>;
  const args = argumentsObject(call.arguments);
  const moved = resultObject(result);
  const [from, to] = [textOf(moved, 'from') ?? textOf(args, 'from'), textOf(moved, 'to') ?? textOf(args, 'to')];
  const renamed = moved?.renamed;
  return (
    <>
      {from !== null && to !== null ? (
        <div className="flex items-start gap-1.5 text-[11px]">
          <span className="mt-0.5 opacity-60 shrink-0"><FileInput size={12} /></span>
          <span className="min-w-0">
            <code className="font-mono break-all">{directoryPath(from)}</code>
            <span className="opacity-60"> → </span>
            <code className="font-mono break-all">{directoryPath(to)}</code>
            {typeof renamed === 'boolean' && <span className="opacity-60"> · {renamed ? 'renamed' : 'moved'}</span>}
          </span>
        </div>
      ) : (
        <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>
      )}
      <FailedResult result={result} />
    </>
  );
};

/** The questions of a call's arguments, or `null` if they are no list of questions. */
function questionsOf(args: Record<string, unknown> | null): Question[] | null {
  const questions = args?.questions;
  if (!Array.isArray(questions)) return null;
  return questions.every(q => typeof q?.header === 'string' && typeof q?.question === 'string') ? questions as Question[] : null;
}

/**
 * Questions put to users (issue 268): nothing raw. While they wait, the questions as they were put
 * and whom they wait for - the card is where they are answered -; once answered, nothing of the
 * call's own, since the frame's record of the answers says it all. A call that failed shows why.
 */
const QuestionsToolCallContent: React.FC<ToolCallContentProps> = ({call, result}) => {
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">Questions not shown</span>;
  if (result?.answers) return null;
  if (result && result.status && result.status !== 'ok') return <FailedResult result={result} />;
  const args = argumentsObject(call.arguments);
  const questions = questionsOf(args);
  if (questions === null) return <code className="text-[11px] font-mono break-all">{formatArguments(call.arguments)}</code>;
  const asked = Array.isArray(args?.ask) ? args.ask.filter((a): a is string => typeof a === 'string') : [];
  return (
    <div className="space-y-3">
      {questions.map((question, index) => (
        <div key={index} className="space-y-1">
          <QuestionHeading question={question} compact />
          {question.choices?.length ? <ChoiceRows question={question} chosen={[]} compact /> : null}
        </div>
      ))}
      <div className="text-[11px] italic opacity-60">
        {result ? 'Answered' : `Waiting for ${asked.join(', ') || 'whoever was asked'} to answer`}
      </div>
    </div>
  );
};

/** Where a file offered for download stands now (issue 273), as far as this user can tell. */
type OfferedState = 'checking' | 'current' | 'changed' | 'gone' | 'unavailable';

/** Whether the file at `path` is as it was offered - its size and when it was changed - or changed, or gone. */
function useOfferedState(files: WorkspaceFiles | undefined, path: string | null, size: number | null, modified: string | null, version: number | undefined): OfferedState {
  const [state, setState] = useState<OfferedState>('checking');
  useEffect(() => {
    if (!files || path === null) return;
    let current = true;
    const cut = path.lastIndexOf('/');
    const [dir, name] = cut < 0 ? ['', path] : [path.slice(0, cut), path.slice(cut + 1)];
    files.list(dir)
      .then(entries => {
        if (!current) return;
        const entry = entries.find(e => e.name === name && !e.is_dir);
        const sameTime = (entry?.modified ? Date.parse(entry.modified) : null) === (modified ? Date.parse(modified) : null);
        setState(!entry ? 'gone' : entry.size === size && sameTime ? 'current' : 'changed');
      })
      // The directory gone is the file gone; no workspace to use here, or any other refusal, is the workspace not being this user's to use.
      .catch(error => {
        if (!current) return;
        const missing = error instanceof ApiRequestError && error.status === 404 && error.code !== 'workspace_not_found';
        setState(missing ? 'gone' : 'unavailable');
      });
    return () => { current = false; };
  }, [files, path, size, modified, version]);
  return files ? state : 'unavailable';
}

/**
 * A file of the workspace offered for download (issue 273): a card with its name, size and what it
 * is, and a download of the file as it is now - saying so where it changed since it was offered, or
 * that it is gone. Whoever cannot use the workspace in the channel sees the card without the download.
 */
const FilePublishToolCallContent: React.FC<ToolCallContentProps> = ({call, result, files, filesVersion}) => {
  const args = argumentsObject(call.arguments);
  const offered = resultObject(result);
  const path = textOf(offered, 'path') ?? textOf(args, 'path');
  const name = textOf(offered, 'name') ?? textOf(args, 'name') ?? (path?.split('/').pop() || null);
  const size = numberOf(offered, 'size');
  const state = useOfferedState(offered ? files : undefined, offered ? path : null, size, textOf(offered, 'modified'), filesVersion);
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">File not shown</span>;
  if (!offered) {
    return result ? <FailedResult result={result} /> : <FilePath icon={<FileDown size={12} />} path={path} detail="being offered" />;
  }
  const notes: Record<OfferedState, string | null> = {
    checking: null,
    current: null,
    changed: 'Changed since it was offered: the download is the file as it is now.',
    gone: 'No longer in the workspace.',
    unavailable: files ? 'You cannot use the workspace in this channel.' : 'Download it from the chat.',
  };
  const downloadable = files && path !== null && (state === 'current' || state === 'changed' || state === 'checking');
  return (
    <DownloadCard
      icon={<FileDown size={20} />}
      name={name ?? '?'}
      details={[size !== null ? formatSize(size) : null, path]}
      description={textOf(offered, 'description')}
      note={notes[state]}
      warning={state === 'changed'}
      href={downloadable ? files.url(path) : null}
    />
  );
};

/**
 * A card of something offered for download (issues 271, 273): an icon, its name, what is known of
 * it, what it is, a note on it, the download where there is one, and further actions.
 */
const DownloadCard: React.FC<{
  icon: React.ReactNode;
  name: string;
  details: (string | null)[];
  description?: string | null;
  note?: string | null;
  /** Whether the note warns rather than informs. */
  warning?: boolean;
  href?: string | null;
  actions?: React.ReactNode;
}> = ({icon, name, details, description, note, warning, href, actions}) => (
  <div className="flex items-start gap-3 rounded-md border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-950 p-2.5">
    <span className="shrink-0 mt-0.5 text-blue-600 dark:text-blue-400">{icon}</span>
    <div className="min-w-0 flex-1">
      <div className="text-sm font-medium break-all">{name}</div>
      <div className="text-[11px] opacity-60">{details.filter(Boolean).join(' · ')}</div>
      {description && <div className="text-xs mt-1 whitespace-pre-wrap">{description}</div>}
      {note && <div className={`text-[11px] mt-1 ${warning ? 'text-amber-600 dark:text-amber-400' : 'italic opacity-60'}`}>{note}</div>}
    </div>
    <div className="shrink-0 flex flex-col items-end gap-1.5">
      {href && (
        <a href={href} download={name} className="flex items-center gap-1 rounded-md bg-blue-600 hover:bg-blue-700 text-white text-xs px-2.5 py-1.5">
          <Download size={12} /> Download
        </a>
      )}
      {actions}
    </div>
  </div>
);

/** The user's own artifact `id` as they see it - `null` if it is not theirs, or not known yet - and keeping it. */
function useOwnedArtifact(id: string | null): [ArtifactResponse | null, () => Promise<void>] {
  const [owned, setOwned] = useState<ArtifactResponse | null>(null);
  useEffect(() => {
    if (id === null) return;
    let current = true;
    // Refused for whoever is not its owner: then there is nothing to keep.
    api.getArtifact(id).then(found => { if (current) setOwned(found); }).catch(() => { if (current) setOwned(null); });
    return () => { current = false; };
  }, [id]);
  const keep = async () => {
    if (id === null) return;
    try {
      setOwned(await api.keepArtifact(id, 'user'));
    } catch (error) {
      reportError('Failed to keep the artifact', error);
    }
  };
  return [owned, keep];
}

/**
 * A file published as a version of an artifact (issue 271): a card with its name, version and size,
 * a download of that version - fixed, whatever became of the file -, and for its owner "Keep" while
 * it lies in the session's scope, so that it outlives the session.
 */
const ArtifactPublishToolCallContent: React.FC<ToolCallContentProps> = ({call, result, artifacts}) => {
  const args = argumentsObject(call.arguments);
  const published = resultObject(result);
  const id = textOf(published, 'artifact');
  const version = numberOf(published, 'version');
  const [owned, keep] = useOwnedArtifact(id);
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">File not shown</span>;
  const path = textOf(published, 'path') ?? textOf(args, 'path');
  if (!published) {
    return result ? <FailedResult result={result} /> : <FilePath icon={<Package size={12} />} path={path} detail="being published" />;
  }
  const size = numberOf(published, 'size');
  const kept = owned && owned.scope !== 'session' ? (owned.scope === 'global' ? 'Kept for everyone.' : 'Kept in your artifacts.') : null;
  return (
    <DownloadCard
      icon={<Package size={20} />}
      name={textOf(published, 'name') ?? '?'}
      details={[version !== null ? `version ${version}` : null, size !== null ? formatSize(size) : null, path]}
      description={textOf(published, 'description')}
      note={kept ?? (artifacts ? null : 'Download it from the chat.')}
      href={artifacts && id !== null && version !== null ? artifacts.url(id, version) : null}
      actions={owned?.can_change && owned.scope === 'session' && (
        <button
          type="button"
          onClick={() => void keep()}
          title="Keep it beyond this session, among your artifacts"
          className="flex items-center gap-1 rounded-md border border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-800 text-xs px-2.5 py-1"
        >
          <PackageCheck size={12} /> Keep
        </button>
      )}
    />
  );
};

/**
 * A skill loaded, written or changed (issue 281): its name - linked to its page where the result
 * names the skill and the chat can open it -, what it is for and its text, rendered as Markdown,
 * where the arguments hold them. A loaded skill's text, the result, is never shown: it is the
 * agent's to follow.
 */
const SkillToolCallContent: React.FC<ToolCallContentProps> = ({call, result, onSelectPage}) => {
  if (call.arguments === null) return <span className="text-[11px] italic opacity-60">Skill not shown</span>;
  const args = argumentsObject(call.arguments);
  const answered = resultObject(result);
  const name = textOf(answered, 'name') ?? textOf(args, 'name');
  const skillId = textOf(answered, 'skill_id');
  const status = textOf(answered, 'status');
  const description = textOf(args, 'description');
  const body = textOf(args, 'body');
  return (
    <>
      <div className="flex items-start gap-1.5 text-[11px]">
        <BookOpen size={12} className="mt-0.5 opacity-60 shrink-0" />
        <span className="min-w-0">
          {skillId !== null && onSelectPage ? (
            <button type="button" onClick={() => onSelectPage({kind: 'skill', id: skillId})} title="Open the skill" className="font-medium text-blue-600 dark:text-blue-400 hover:underline break-all">
              {name ?? '?'}
            </button>
          ) : (
            <span className="font-medium break-all">{name ?? '?'}</span>
          )}
          {status && <span className="opacity-60"> · {status}</span>}
        </span>
      </div>
      {description !== null && <div className="mt-1 text-xs">{description}</div>}
      {body !== null && (
        <div className="mt-1 max-h-80 overflow-y-auto p-1.5 rounded-sm border bg-white dark:bg-gray-950 border-gray-100 dark:border-gray-900">
          <MarkdownMessage content={body} className="text-xs" />
        </div>
      )}
      <FailedResult result={result} />
    </>
  );
};

/** The handler of each presentation; one the server knows and this client does not falls back to the default. */
const PRESENTATIONS: Record<ToolPresentation, React.FC<ToolCallContentProps>> = {
  default: DefaultToolCallContent,
  shell: ShellToolCallContent,
  plan: PlanToolCallContent,
  file_read: FileReadToolCallContent,
  file_write: FileWriteToolCallContent,
  file_edit: FileEditToolCallContent,
  file_list: FileListToolCallContent,
  file_search: FileSearchToolCallContent,
  file_move: FileMoveToolCallContent,
  questions: QuestionsToolCallContent,
  file_publish: FilePublishToolCallContent,
  artifact_publish: ArtifactPublishToolCallContent,
  skill: SkillToolCallContent,
};

interface ToolCallViewProps {
  call: ToolCall;
  /** Its result, once it has one. */
  result?: ToolResult;
  /** Its caller was removed from the channel before it ended. */
  abandoned?: boolean;
  /** The files of the channel's workspace, for a call that offers one (issue 273). */
  files?: WorkspaceFiles;
  /** Changes whenever those files may have changed. */
  filesVersion?: number;
  /** The artifacts of the channel, for a call that published one (issue 271). */
  artifacts?: ChannelArtifacts;
  /** Opens a page in the place of the channel - a skill's, for a call that names one (issue 281). */
  onSelectPage?: (page: Page) => void;
  /** Interrupts the call alone while it runs, for who may (issue 276); the turn goes on. */
  onInterrupt?: () => void;
  /** Ends a command the call handed back as running, for who may (issue 294). */
  onKillCommand?: (commandId: string) => void;
  /** The commands ended already, which are not offered to be ended. */
  endedCommands?: ReadonlySet<string>;
}

export const ToolCallView: React.FC<ToolCallViewProps> = ({call, result, abandoned, files, filesVersion, artifacts, onSelectPage, onInterrupt, onKillCommand, endedCommands}) => {
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
            {onInterrupt && (
              <button
                type="button"
                onClick={onInterrupt}
                className="ml-1 p-0.5 rounded-sm text-gray-400 hover:text-red-600 dark:hover:text-red-400 transition-colors"
                title="Interrupt this call; the agent goes on"
                aria-label="Interrupt this call"
              >
                <Square size={10} fill="currentColor" />
              </button>
            )}
          </div>
        )}
        </div>
      </div>
      <div className="p-2">
        <Content call={call} result={result} files={files} filesVersion={filesVersion} artifacts={artifacts} onSelectPage={onSelectPage} onKillCommand={onKillCommand} endedCommands={endedCommands} />
        {result?.answers && (
          // Set apart from what the presentation shows, where it shows anything.
          <div className="not-first:mt-2 not-first:pt-2 not-first:border-t border-gray-200 dark:border-gray-700">
            <AnswersRecord answers={result.answers} />
          </div>
        )}
      </div>
    </div>
  );
};
