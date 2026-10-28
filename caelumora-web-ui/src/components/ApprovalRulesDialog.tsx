// The approval rules of a session or a channel (issue 13): what it restricts
// of the tool calls its agents make - a rule asks a person to decide the call
// themselves, or denies it outright. A place never allows; that is a
// person's to say. An argument is compared by a pattern, as a bash command
// line by the words its commands begin with (issue 247), or as it is (issue
// 400). Whether the rules
// may be changed, and whether a pattern is one, is the server's to decide.

import React, {useState} from 'react';
import {Plus, Trash2, X} from 'lucide-react';
import type {ApprovalRule, ArgumentMatcher, RuleDecision} from '../types';
import {reportUnlessRefused} from '../errors';

interface ApprovalRulesDialogProps {
  title: string;
  /** What the place is called in the explanation: "session" or "channel". */
  place: string;
  current: ApprovalRule[];
  canChange: boolean;
  onSubmit: (rules: ApprovalRule[]) => Promise<void>;
  onClose: () => void;
}

/** A rule as it is edited: the arguments as rows, so that one can be added before it is named. */
interface Draft {
  id?: string;
  tool: string;
  decision: RuleDecision;
  arguments: ArgumentDraft[];
}

/** An argument as it is edited: a pattern, the words of a command line separated by blanks, or a value. */
interface ArgumentDraft {
  name: string;
  kind: 'pattern' | 'shell' | 'equals';
  text: string;
}

function argumentDraftOf(name: string, argument: ArgumentMatcher): ArgumentDraft {
  if (typeof argument === 'string') return {name, kind: 'pattern', text: argument};
  if ('shell' in argument) return {name, kind: 'shell', text: argument.shell.join(' ')};
  return {name, kind: 'equals', text: argument.equals};
}

function argumentOf(draft: ArgumentDraft): ArgumentMatcher {
  switch (draft.kind) {
    case 'pattern': return draft.text;
    case 'shell': return {shell: draft.text.split(/\s+/).filter(word => word !== '')};
    case 'equals': return {equals: draft.text};
  }
}

const ARGUMENT_PLACEHOLDERS: Record<ArgumentDraft['kind'], string> = {
  pattern: 'pattern',
  shell: 'words, e.g. git push',
  equals: 'the whole value',
};

const DECISIONS: {value: RuleDecision; label: string}[] = [
  {value: 'ask', label: 'Ask - a person decides each call'},
  {value: 'deny', label: 'Deny - never run'},
];

function draftOf(rule: ApprovalRule): Draft {
  return {
    id: rule.id,
    tool: rule.matcher.tool,
    decision: rule.decision,
    arguments: Object.entries(rule.matcher.arguments ?? {}).map(([name, argument]) => argumentDraftOf(name, argument)),
  };
}

function ruleOf(draft: Draft): ApprovalRule {
  const named = draft.arguments.filter(a => a.name.trim() !== '');
  return {
    id: draft.id,
    matcher: {tool: draft.tool.trim(), arguments: Object.fromEntries(named.map(a => [a.name.trim(), argumentOf(a)]))},
    decision: draft.decision,
  };
}

export const ApprovalRulesDialog: React.FC<ApprovalRulesDialogProps> = ({title, place, current, canChange, onSubmit, onClose}) => {
  const [drafts, setDrafts] = useState<Draft[]>(() => current.map(draftOf));
  const [problem, setProblem] = useState<string | null>(null);
  const [isSaving, setIsSaving] = useState(false);
  const complete = drafts.every(d => d.tool.trim() !== '');

  const change = (index: number, update: (draft: Draft) => Draft) =>
    setDrafts(prev => prev.map((d, i) => i === index ? update(d) : d));

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canChange || !complete || isSaving) return;
    setIsSaving(true);
    setProblem(null);
    try {
      await onSubmit(drafts.map(ruleOf));
      onClose();
    } catch (error) {
      // What the server says about the rules - a pattern that is none, say -
      // is shown here; anything else as usual.
      reportUnlessRefused('Failed to change the approval rules', error, setProblem);
    } finally {
      setIsSaving(false);
    }
  };

  const field = 'rounded-md border border-gray-700 bg-gray-800 px-2 py-1 text-sm disabled:opacity-60';

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
      onClick={onClose}
      onKeyDown={(e) => { if (e.key === 'Escape') onClose(); }}
    >
      <form
        onSubmit={submit}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="approval-rules-title"
        className="flex max-h-full w-full max-w-lg flex-col rounded-lg border border-gray-700 bg-gray-900 text-gray-100 shadow-xl"
      >
        <div className="flex items-center gap-2 border-b border-gray-800 p-4">
          <h2 id="approval-rules-title" className="flex-1 font-semibold text-sm truncate">{title}</h2>
          <button type="button" onClick={onClose} className="p-1 hover:bg-gray-800 rounded-md" title="Close">
            <X size={16} />
          </button>
        </div>

        <div className="space-y-4 overflow-y-auto p-4 text-sm">
          <p className="text-xs text-gray-400">
            What the {place} restricts of the tool calls its agents make, whatever the tool or anyone's own rules
            say. A tool is named as the agent calls it, <code>*</code> for every tool; a pattern is a regular
            expression the whole value of the argument must match. An argument that is a command line is better
            compared by the words its commands begin with: a rule on <code>rm</code> also finds it
            in <code>ls; rm x</code>, and asks for a line it cannot read.
          </p>
          {drafts.length === 0 && <p className="text-xs text-gray-500">No rules: each tool decides whether it asks.</p>}
          {drafts.map((draft, index) => (
            <fieldset key={index} disabled={!canChange} className="space-y-2 rounded-md border border-gray-800 p-3">
              <div className="flex items-center gap-2">
                <input
                  aria-label="Tool"
                  placeholder="tool, or *"
                  value={draft.tool}
                  onChange={(e) => change(index, d => ({...d, tool: e.target.value}))}
                  className={`${field} min-w-0 flex-1 font-mono`}
                />
                <select
                  aria-label="Decision"
                  value={draft.decision}
                  onChange={(e) => change(index, d => ({...d, decision: e.target.value as RuleDecision}))}
                  className={field}
                >
                  {DECISIONS.map(d => <option key={d.value} value={d.value}>{d.label}</option>)}
                </select>
                {canChange && (
                  <button type="button" onClick={() => setDrafts(prev => prev.filter((_, i) => i !== index))} className="p-1 hover:bg-gray-800 rounded-md" title="Remove the rule">
                    <Trash2 size={14} />
                  </button>
                )}
              </div>
              {draft.arguments.map((argument, a) => (
                <div key={a} className="flex items-center gap-2 pl-4">
                  <input
                    aria-label="Argument"
                    placeholder="argument"
                    value={argument.name}
                    onChange={(e) => change(index, d => ({...d, arguments: d.arguments.map((x, i) => i === a ? {...x, name: e.target.value} : x)}))}
                    className={`${field} w-32 font-mono`}
                  />
                  <select
                    aria-label="Compared as"
                    value={argument.kind}
                    onChange={(e) => change(index, d => ({...d, arguments: d.arguments.map((x, i) => i === a ? {...x, kind: e.target.value as ArgumentDraft['kind']} : x)}))}
                    className={field}
                  >
                    <option value="pattern">matches</option>
                    <option value="shell">commands begin with</option>
                    <option value="equals">is exactly</option>
                  </select>
                  <input
                    aria-label="Value compared"
                    placeholder={ARGUMENT_PLACEHOLDERS[argument.kind]}
                    value={argument.text}
                    onChange={(e) => change(index, d => ({...d, arguments: d.arguments.map((x, i) => i === a ? {...x, text: e.target.value} : x)}))}
                    className={`${field} min-w-0 flex-1 font-mono`}
                  />
                  {canChange && (
                    <button type="button" onClick={() => change(index, d => ({...d, arguments: d.arguments.filter((_, i) => i !== a)}))} className="p-1 hover:bg-gray-800 rounded-md" title="Remove the argument">
                      <X size={14} />
                    </button>
                  )}
                </div>
              ))}
              {canChange && (
                <button type="button" onClick={() => change(index, d => ({...d, arguments: [...d.arguments, {name: '', kind: 'pattern', text: ''}]}))} className="ml-4 text-xs text-blue-400 hover:underline">
                  Only when an argument matches…
                </button>
              )}
            </fieldset>
          ))}
          {canChange && (
            <button
              type="button"
              onClick={() => setDrafts(prev => [...prev, {tool: '', decision: 'ask', arguments: []}])}
              className="flex items-center gap-1 text-xs text-blue-400 hover:underline"
            >
              <Plus size={14} /> Add a rule
            </button>
          )}
          {problem && <div role="alert" className="text-xs text-red-400">{problem}</div>}
        </div>

        <div className="flex justify-end gap-2 border-t border-gray-800 p-4">
          <button type="button" onClick={onClose} className="rounded-md px-3 py-1 text-sm hover:bg-gray-800">
            {canChange ? 'Cancel' : 'Close'}
          </button>
          {canChange && (
            <button
              type="submit"
              disabled={!complete || isSaving}
              className="rounded-md bg-blue-600 px-3 py-1 text-sm text-white hover:bg-blue-700 disabled:opacity-50"
            >
              {isSaving ? 'Saving…' : 'Save'}
            </button>
          )}
        </div>
      </form>
    </div>
  );
};
