// What the UI shows of approval rules (issue 13). Whether a rule applies is
// the server's to decide; this only says what a rule is about, and which of
// the user's own rules approve everything here, to show while they do.

import type {ApprovalRequest, ApprovalRule, ArgumentMatcher, Reach, ToolMatcher} from './types';

/** Whether `rule` reaches the channel `channelId` of session `sessionId`. */
export function inReach(rule: ApprovalRule, sessionId: string, channelId: string): boolean {
  const reach: Reach = rule.reach ?? 'everywhere';
  if (reach === 'everywhere') return true;
  return 'session' in reach ? reach.session === sessionId : reach.channel === channelId;
}

/** Whether `rule` still applies at `now`. */
export function isActive(rule: ApprovalRule, now: Date = new Date()): boolean {
  return !rule.expires_at || new Date(rule.expires_at) > now;
}

/** Whether `matcher` is about every call of every tool. */
export function isEverything(matcher: ToolMatcher): boolean {
  return matcher.tool === '*' && Object.keys(matcher.arguments ?? {}).length === 0;
}

/**
 * The user's rule that approves every call here, if one does - the "brave
 * mode": shown for as long as it lasts, so that it is not left on unseen.
 */
export function approvingEverything(rules: ApprovalRule[], sessionId: string, channelId: string): ApprovalRule | null {
  return rules.find(r => r.decision === 'allow' && isEverything(r.matcher) && inReach(r, sessionId, channelId) && isActive(r)) ?? null;
}

/** The rule for the brave mode: every call in session `sessionId`, for `minutes`. */
export function approveEverything(sessionId: string, minutes: number): ApprovalRule {
  return {
    matcher: {tool: '*'},
    decision: 'allow',
    reach: {session: sessionId},
    expires_at: new Date(Date.now() + minutes * 60_000).toISOString(),
  };
}

/** What `matcher` is about, in a line: the tool, and each argument as it is compared. */
export function describeMatcher(matcher: ToolMatcher): string {
  const tool = matcher.tool === '*' ? 'every tool' : matcher.tool;
  const args = Object.entries(matcher.arguments ?? {}).map(([name, argument]) => `${name}${describeArgument(argument)}`);
  return args.length === 0 ? tool : `${tool} (${args.join(', ')})`;
}

/**
 * How an argument is compared, after its name: ` ~ pattern`, `: the words …` of a command line, or
 * ` = "value"`.
 */
function describeArgument(argument: ArgumentMatcher): string {
  if (typeof argument === 'string') return ` ~ ${argument}`;
  if ('shell' in argument) return `: ${argument.shell.join(' ')} …`;
  return ` = "${shortened(argument.equals, 40)}"`;
}

/** `text`, cut to `length` characters with an ellipsis if longer. */
function shortened(text: string, length: number): string {
  return text.length <= length ? text : `${text.slice(0, length)}…`;
}

/** Where `rule` reaches, in words; the session's or channel's name if known. */
export function describeReach(rule: ApprovalRule, nameOf: (reach: Reach) => string | null): string {
  const reach = rule.reach ?? 'everywhere';
  if (reach === 'everywhere') return 'everywhere';
  const name = nameOf(reach);
  return 'session' in reach ? `in session ${name ?? '(gone)'}` : `in channel ${name ?? '(gone)'}`;
}

/** What the user chose to add with their answer; `null` for no rule. */
export interface RuleChoice {
  matcher: ToolMatcher;
  reach: 'channel' | 'session' | 'everywhere';
  /** For how many minutes; `null` for no end. */
  minutes: number | null;
}

/**
 * The choice a card starts with: the calls `suggested` matches - as the call's tool proposes -, in
 * this channel, for no end.
 */
export function initialChoice(suggested: ToolMatcher): RuleChoice {
  return {matcher: suggested, reach: 'channel', minutes: null};
}

/** The rule `choice` makes, deciding as the user answered. */
export function ruleOf(choice: RuleChoice, approved: boolean, request: ApprovalRequest): ApprovalRule {
  const reach: Reach = choice.reach === 'everywhere' ? 'everywhere'
    : choice.reach === 'session' ? {session: request.session_id} : {channel: request.channel_id};
  return {
    matcher: choice.matcher,
    decision: approved ? 'allow' : 'deny',
    reach,
    expires_at: choice.minutes === null ? undefined : new Date(Date.now() + choice.minutes * 60_000).toISOString(),
  };
}
