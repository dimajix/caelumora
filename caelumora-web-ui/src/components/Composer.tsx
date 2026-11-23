// The field a user writes in a conversation (issue 595): completion of mentions and commands, the
// classification, what is attached, and sending it or stopping what is generated.

import React, {useRef, useState} from 'react';
import {Send, Square} from 'lucide-react';
import type {ChannelParticipant, CommandDescription} from '../types';
import {sameClassification, type Classification} from '../confidentiality';
import {ClassificationPicker} from './ClassificationPicker';
import {AttachButton} from './AttachButton';
import {AttachmentDraftView} from './AttachmentDraftView';
import {answersOnlyWhenMentioned} from '../moderator';
import {reportError} from '../errors';
import {cn} from '../classNames';
import {GrowingTextarea} from './GrowingTextarea';
import {GROUP_HANDLES, handleOf, mentionQueryAt} from '../mention';
import {commandQueryAt, completes, parseInput, usageOf, type UserInput} from '../command';
import {useChatChannel} from '../chatChannel';
import type {ComposerState} from '../composer';

type MentionableParticipant = ChannelParticipant & {handle: string};

/** An entry of the completion list: a participant or everyone to mention, or a command to run. */
// A group: a handle that mentions every participant (issue 401). A command suggested: one of the
// channel, or one an agent carries out (issue 269), with the handle it is written with - `all` for
// every agent that carries it out.
type Suggestion =
  | {kind: 'mention'; participant: MentionableParticipant}
  | {kind: 'group'; handle: string}
  | {kind: 'command'; command: CommandDescription; handle?: string};

interface ComposerProps {
  /** What is being written, held where files dropped onto the conversation reach it as well. */
  composer: ComposerState;
  /**
   * Sends `input` - a message with what is `attached` to it, classified `release`, or a command -;
   * fails with why, and what was typed is given back.
   */
  onSend: (input: UserInput, attached: string[], release: Classification | null) => Promise<void>;
  /** Stops what is generated; offered while something is. */
  onStop: () => void;
  /** Whether an answer is being generated in the conversation it writes in. */
  generating: boolean;
}

/**
 * The field a user writes in (issue 595): completion of mentions and commands, the classification,
 * what is attached; sends through `onSend`, and stops through `onStop` while something generates.
 */
export const Composer: React.FC<ComposerProps> = ({composer, onSend, onStop, generating}) => {
  const {channel, participants, commands, compartments} = useChatChannel();
  const {text: inputText, setText: setInputText, draft, release, setRelease} = composer;
  const [isSending, setIsSending] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  // `@`-completion: the caret position the suggestions were computed for, the highlighted
  // suggestion, and the query the user dismissed with Escape.
  const [caret, setCaret] = useState(0);
  const [suggestionIndex, setSuggestionIndex] = useState(0);
  const [dismissedMention, setDismissedMention] = useState<number | null>(null);

  const mentionQuery = mentionQueryAt(inputText, caret);
  const typedCommand = commandQueryAt(inputText, caret);
  // The commands the agents here carry out (issue 269), by name, with the handles of the agents
  // that do: one is suggested for each of them, and for `@all` where several do.
  const agentCommands = new Map<string, {command: CommandDescription; handles: string[]}>();
  for (const participant of participants) {
    if (!participant.handle || participant.left) continue;
    for (const command of participant.commands ?? []) {
      const entry = agentCommands.get(command.name) ?? {command, handles: []};
      entry.handles.push(participant.handle);
      agentCommands.set(command.name, entry);
    }
  }
  const agentSuggestions = [...agentCommands.values()].flatMap(({command, handles}) =>
    [...handles, ...(handles.length > 1 ? [GROUP_HANDLES[0]] : [])].map((handle): Suggestion => ({kind: 'command', command, handle})));
  // A command's name has words (issue 560), so what is typed after the slash is a command being
  // typed only while a command's name begins with it; past that, a mention may be.
  const commandSuggestions = typedCommand === null
    ? []
    : [...commands.map((command): Suggestion => ({kind: 'command', command})), ...agentSuggestions]
        .filter(s => s.kind === 'command' && completes(s.command, typedCommand));
  const commandQuery = commandSuggestions.length > 0 ? typedCommand : null;
  // Where the completed token starts; a command always starts at 0.
  const suggestionStart = commandQuery !== null ? 0 : mentionQuery?.start ?? null;
  const mentionText = (mentionQuery?.query ?? '').toLowerCase();
  const suggestions: Suggestion[] = suggestionStart === null || suggestionStart === dismissedMention
    ? []
    : commandQuery !== null
      ? commandSuggestions
      : [
          ...participants
            .filter((m): m is MentionableParticipant =>
              !!m.handle && (m.handle.startsWith(handleOf(mentionText)) || m.name.toLowerCase().includes(mentionText)))
            .map((participant): Suggestion => ({kind: 'mention', participant})),
          ...GROUP_HANDLES
            .filter(handle => handle.startsWith(handleOf(mentionText)))
            .map((handle): Suggestion => ({kind: 'group', handle})),
        ];

  // What is attached to the message being written, by id; `null` while something still uploads.
  const attached = draft.ids();
  const mayBeSent = !isSending && attached !== null && (!!inputText.trim() || draft.items.length > 0);

  const handleSend = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!mayBeSent || attached === null) return;
    const text = inputText;
    // What is sent; anything attached meanwhile stays for the next message.
    const sent = draft.items.map(item => item.key);
    setInputText('');
    setIsSending(true);
    const input = parseInput(text);
    try {
      await onSend(input, attached, release);
      if (input.kind === 'message') {
        // A release is for one message, and so is what was attached.
        setRelease(null);
        draft.remove(...sent);
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

  const updateInput = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setInputText(e.target.value);
    setCaret(e.target.selectionStart ?? e.target.value.length);
    setSuggestionIndex(0);
  };

  const selectCommand = (command: CommandDescription, handle?: string) => {
    // The rest of the name typed, and the blanks after it - not a line break: the rest of the word
    // at the caret, and the words after it as long as the name goes on with them (issue 560).
    let typed = inputText.slice(0, caret);
    let rest = inputText.slice(caret);
    for (let first = true; ; first = false) {
      const word = /^\S*[^\S\n]*/.exec(rest)?.[0] ?? '';
      const query = commandQueryAt(typed + word, (typed + word).length);
      if (!word || (!first && (query === null || !completes(command, query.trimEnd())))) break;
      typed += word;
      rest = rest.slice(word.length);
    }
    const inserted = handle ? `/${command.name} @${handle} ` : `/${command.name} `;
    const newCaret = inserted.length;
    setInputText(inserted + rest);
    setCaret(newCaret);
    setSuggestionIndex(0);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.setSelectionRange(newCaret, newCaret);
    });
  };

  const selectMention = (handle: string) => {
    if (!mentionQuery) return;
    const before = inputText.slice(0, mentionQuery.start);
    const after = inputText.slice(caret).replace(/^\s*/, ' ');
    const inserted = `@${handle}`;
    const newCaret = before.length + inserted.length + 1;
    setInputText(before + inserted + after);
    setCaret(newCaret);
    setSuggestionIndex(0);
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.setSelectionRange(newCaret, newCaret);
    });
  };

  const selectSuggestion = (suggestion: Suggestion) => {
    if (suggestion.kind === 'command') selectCommand(suggestion.command, suggestion.handle);
    else selectMention(suggestion.kind === 'group' ? suggestion.handle : suggestion.participant.handle);
  };

  const handleInputKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // A key that ends an input method's composition is the composition's.
    if (e.nativeEvent.isComposing) return;
    // Shift+Enter is a line break, also while completing (issue 274).
    const enter = e.key === 'Enter' && !e.shiftKey;
    if (suggestions.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSuggestionIndex(i => (i + 1) % suggestions.length);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSuggestionIndex(i => (i - 1 + suggestions.length) % suggestions.length);
      } else if (enter || e.key === 'Tab') {
        e.preventDefault();
        selectSuggestion(suggestions[Math.min(suggestionIndex, suggestions.length - 1)]);
      } else if (e.key === 'Escape') {
        e.preventDefault();
        setDismissedMention(suggestionStart);
      }
    } else if (enter) {
      e.preventDefault();
      e.currentTarget.form?.requestSubmit();
    }
  };

  return (
    <>
      <AttachmentDraftView draft={draft} channelClassification={channel?.classification} />
      <form
        onSubmit={handleSend}
        // The controls beside the field stay one line of it high, at its bottom, while it grows
        // (issue 274).
        className="flex items-end space-x-2"
      >
        {channel && (
          <div className="h-10.5 flex">
            <ClassificationPicker
              channel={channel.classification}
              compartments={compartments}
              value={release ?? channel.classification}
              onChange={(c) => setRelease(sameClassification(c, channel.classification) ? null : c)}
            />
          </div>
        )}
        <div className="h-10.5 flex">
          <AttachButton onFiles={(files) => draft.add(files, release ?? undefined)} />
        </div>
        <div className="relative flex-1 flex">
          {suggestions.length > 0 && (
            <ul
              role="listbox"
              className="absolute bottom-full left-0 mb-1 w-64 max-h-60 overflow-y-auto z-10 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-600 rounded-md shadow-lg py-1"
            >
              {suggestions.map((suggestion, idx) => {
                const [label, detail, description] = suggestion.kind === 'command'
                  ? [`/${suggestion.command.name}`, [suggestion.handle && `@${suggestion.handle}`, usageOf(suggestion.command)].filter(Boolean).join(' '), suggestion.command.description]
                  : suggestion.kind === 'group'
                    ? ['Everyone', `@${suggestion.handle}`, 'Every participant of this channel']
                    : [suggestion.participant.name, `@${suggestion.participant.handle}`, null];
                return (
                  <li
                    key={`${suggestion.kind}:${label}:${suggestion.kind !== 'mention' ? suggestion.handle ?? '' : ''}`}
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
          <GrowingTextarea
            ref={inputRef}
            value={inputText}
            onChange={updateInput}
            onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? inputText.length)}
            onKeyDown={handleInputKeyDown}
            onPaste={(e) => draft.paste(e, release ?? undefined)}
            onBlur={() => setDismissedMention(suggestionStart)}
            onFocus={() => setDismissedMention(null)}
            placeholder={answersOnlyWhenMentioned(channel?.responder_policy)
              ? "Agents answer only when @-mentioned... (/ for commands)"
              : "Type your message... (@ to mention, / for commands)"}
            // Read-only rather than disabled while sending: a disabled field loses the focus, and
            // the user would have to click back into it for the next message.
            readOnly={isSending}
            aria-busy={isSending}
            role="combobox"
            aria-expanded={suggestions.length > 0}
            aria-autocomplete="list"
            className="flex-1 min-h-10.5 max-h-[33vh] p-2 border border-gray-300 dark:border-gray-600 rounded-md bg-transparent dark:text-white focus:outline-hidden focus:ring-2 focus:ring-blue-500"
          />
        </div>
        <button
          type="submit"
          // Enabled while an agent answers too (issue 202): the kernel takes a message at any
          // time, and an agent in a turn hears it once the turn ends.
          disabled={!mayBeSent}
          className="h-10.5 p-2 bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white rounded-md transition-colors"
        >
          <Send size={20} />
        </button>
        {generating && (
          <button
            type="button"
            onClick={onStop}
            className="h-10.5 p-2 bg-red-600 hover:bg-red-700 text-white rounded-md transition-colors"
            title="Stop generation"
          >
            <Square size={20} fill="currentColor" />
          </button>
        )}
      </form>
    </>
  );
};
