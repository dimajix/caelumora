// The rule an approval card offers to derive from the call (issue 13): "and
// from now on" - the calls like this one, as its tool suggests, within a
// reach and for a while the user picks. Answering the call is the card's;
// this only says which rule, if any, is to be added with the answer.

import React from 'react';
import type {RuleChoice} from '../approvalRules';

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
  const setPattern = (name: string, pattern: string) => choice && onChange({
    ...choice,
    matcher: {...choice.matcher, arguments: {...choice.matcher.arguments, [name]: pattern}},
  });
  const dropPattern = (name: string) => {
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
          {args.map(([name, pattern]) => (
            <div key={name} className="flex items-center gap-2">
              <code className="w-24 shrink-0 truncate" title={name}>{name}</code>
              <input
                aria-label={`Pattern of ${name}`}
                value={pattern}
                onChange={(e) => setPattern(name, e.target.value)}
                className={`${field} min-w-0 flex-1 font-mono`}
              />
              <button type="button" onClick={() => dropPattern(name)} className="text-gray-400 hover:text-gray-600" title="Any value of this argument">
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
          <div className="text-gray-500">The patterns are regular expressions the whole value must match; your answer then counts as your click would.</div>
        </div>
      )}
    </div>
  );
};
