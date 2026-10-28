// The rule an approval card offers to derive from the call (issue 13): "and
// from now on" - the calls like this one, as its tool suggests, within a
// reach and for a while the user picks. Answering the call is the card's;
// this only says which rule, if any, is to be added with the answer. A
// command line is offered by the words it begins with (issue 247), of which
// the user keeps fewer; which words a line has is the server's to read. A
// value too long for a pattern is offered as it is (issue 400), to keep or
// to drop. Where no rule can be derived, none is offered.

import React from 'react';
import type {RuleChoice} from '../approvalRules';
import type {ArgumentMatcher} from '../types';

const EXPIRIES: {label: string; minutes: number | null}[] = [
  {label: 'until I take it back', minutes: null},
  {label: 'for an hour', minutes: 60},
  {label: 'for a day', minutes: 24 * 60},
];

interface ApprovalRuleOfferProps {
  choice: RuleChoice | null;
  initial: RuleChoice;
  onChange: (choice: RuleChoice | null) => void;
}

export const ApprovalRuleOffer: React.FC<ApprovalRuleOfferProps> = ({choice, initial, onChange}) => {
  const field = 'rounded-sm border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 px-1 py-0.5 text-xs';
  const args = Object.entries(choice?.matcher.arguments ?? {});
  const suggested = initial.matcher.arguments ?? {};
  const setArgument = (name: string, argument: ArgumentMatcher) => choice && onChange({
    ...choice,
    matcher: {...choice.matcher, arguments: {...choice.matcher.arguments, [name]: argument}},
  });
  const dropArgument = (name: string) => {
    if (!choice) return;
    const rest = {...choice.matcher.arguments};
    delete rest[name];
    onChange({...choice, matcher: {...choice.matcher, arguments: rest}});
  };

  return (
    <div className="mb-3 text-xs text-gray-600 dark:text-gray-300 space-y-2">
      <label className="flex items-center gap-2">
        <input type="checkbox" checked={choice !== null} onChange={(e) => onChange(e.target.checked ? initial : null)} />
        and from now on, for calls like this one
      </label>
      {choice && (
        <div className="space-y-1 pl-5">
          {args.length === 0 && <div className="text-gray-500">every call of <code>{choice.matcher.tool}</code></div>}
          {args.map(([name, argument]) => (
            <div key={name} className="flex items-center gap-2">
              <code className="w-24 shrink-0 truncate" title={name}>{name}</code>
              {typeof argument === 'string' ? (
                <input
                  aria-label={`Pattern of ${name}`}
                  value={argument}
                  onChange={(e) => setArgument(name, e.target.value)}
                  className={`${field} min-w-0 flex-1 font-mono`}
                />
              ) : 'shell' in argument ? (
                <ShellWordsChoice name={name} words={argument.shell} offered={suggested[name]} field={field} onChange={(shell) => setArgument(name, {shell})} />
              ) : (
                <span className="min-w-0 flex-1 truncate text-gray-500" title={argument.equals}>
                  exactly this value ({argument.equals.length} characters)
                </span>
              )}
              <button type="button" onClick={() => dropArgument(name)} className="text-gray-400 hover:text-gray-600" title="Any value of this argument">
                any
              </button>
            </div>
          ))}
          <div className="flex flex-wrap items-center gap-2">
            <select aria-label="Where" value={choice.reach} onChange={(e) => onChange({...choice, reach: e.target.value as RuleChoice['reach']})} className={field}>
              <option value="channel">in this channel</option>
              <option value="session">in this session</option>
              <option value="everywhere">everywhere</option>
            </select>
            <select aria-label="How long" value={choice.minutes ?? ''} onChange={(e) => onChange({...choice, minutes: e.target.value === '' ? null : Number(e.target.value)})} className={field}>
              {EXPIRIES.map(x => <option key={x.label} value={x.minutes ?? ''}>{x.label}</option>)}
            </select>
          </div>
          <div className="text-gray-500">
            The patterns are regular expressions the whole value must match; a command line matches if each of its
            commands begins with the words chosen. Your answer then counts as your click would.
          </div>
        </div>
      )}
    </div>
  );
};

interface ShellWordsChoiceProps {
  name: string;
  /** The words chosen. */
  words: string[];
  /** What the call suggested for the argument: its words are those offered. */
  offered: ArgumentMatcher | undefined;
  field: string;
  onChange: (words: string[]) => void;
}

/**
 * How many of the words a command line was suggested with the rule keeps: the commands beginning
 * with them.
 */
const ShellWordsChoice: React.FC<ShellWordsChoiceProps> = ({name, words, offered, field, onChange}) => {
  const all = offered !== undefined && typeof offered !== 'string' && 'shell' in offered ? offered.shell : words;
  return (
    <select
      aria-label={`Words of ${name}`}
      value={words.length}
      onChange={(e) => onChange(all.slice(0, Number(e.target.value)))}
      className={`${field} min-w-0 flex-1 font-mono`}
    >
      {all.map((_, i) => <option key={i} value={i + 1}>{all.slice(0, i + 1).join(' ')} …</option>)}
    </select>
  );
};
