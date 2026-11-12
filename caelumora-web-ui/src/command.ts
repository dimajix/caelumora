// User commands (issue 28). Mirrors `caelumora_domain::command::parse_input`:
// the client decides whether a line is a command or a message, and sends it
// to the matching endpoint.

import type {Command} from './types';

/**
 * A line of user input, classified. A command is the line after the slash: which command it is,
 * and where its arguments begin, the server decides by its commands (issue 560).
 */
export type UserInput =
  | {kind: 'message'; text: string}
  | {kind: 'command'; line: string};

/**
 * Classifies a line of user input. A line starting with `/` is a command, the rest of the line
 * after the slash; `//` escapes a literal slash, so `//etc/hosts` is the message `/etc/hosts`. A
 * slash elsewhere is text.
 */
export function parseInput(input: string): UserInput {
  if (!input.startsWith('/')) return {kind: 'message', text: input};
  const rest = input.slice(1);
  if (rest.startsWith('/')) return {kind: 'message', text: rest};
  return {kind: 'command', line: rest.trim()};
}

/**
 * What is typed of a command, for completion: the text after a leading `/` up to the caret, in
 * lower case, its whitespace one space each, none before it - a command's name has words
 * (issue 560). `null` when not typing a command.
 */
export function commandQueryAt(text: string, caret: number): string | null {
  const match = /^\/([^/]*)$/.exec(text.slice(0, caret));
  return match ? match[1].trimStart().replace(/\s+/g, ' ').toLowerCase() : null;
}

/** Whether `command` is completed from `query`: its name or an alias begins with it. */
export function completes(command: Command, query: string): boolean {
  return [command.name, ...(command.aliases ?? [])].some(name => name.startsWith(query));
}
