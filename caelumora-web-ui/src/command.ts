// User commands (issue 28). Mirrors `caelumora_model::command::parse_input`:
// the client decides whether a line is a command or a message, and sends it
// to the matching endpoint.

/** A line of user input, classified. */
export type UserInput =
  | {kind: 'message'; text: string}
  | {kind: 'command'; name: string; args: string};

/**
 * Classifies a line of user input. A line starting with `/` is a command
 * whose name runs up to the first whitespace; `//` escapes a literal slash,
 * so `//etc/hosts` is the message `/etc/hosts`. A slash elsewhere is text.
 */
export function parseInput(input: string): UserInput {
  if (!input.startsWith('/')) return {kind: 'message', text: input};
  const rest = input.slice(1);
  if (rest.startsWith('/')) return {kind: 'message', text: rest};
  const match = /\s/.exec(rest);
  if (!match) return {kind: 'command', name: rest, args: ''};
  return {kind: 'command', name: rest.slice(0, match.index), args: rest.slice(match.index).trim()};
}

/**
 * The command name being typed, for completion: the text after a leading
 * `/` while the caret is still in the name. `null` when not typing a command.
 */
export function commandQueryAt(text: string, caret: number): string | null {
  const before = text.slice(0, caret);
  const match = /^\/([^\s/]*)$/.exec(before);
  return match ? match[1] : null;
}
