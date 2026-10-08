// The questions of an agent's tool call that wait for an answer (issue 165): shown to everybody
// they reach, with controls only for those they are put to. Each question is answered as it asks -
// text, or choosing among its choices, one or several, or writing another answer - or skipped;
// the whole of them may be declined.

import React, {useState} from 'react';
import {HelpCircle, Lock} from 'lucide-react';
import type {Question, QuestionRequest, QuestionResponse} from '../types';
import {type ClassValue, clsx} from 'clsx';
import {twMerge} from 'tailwind-merge';

function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/** What the user has given so far for one question. */
interface Draft {
  chosen: string[];
  other: string;
  text: string;
  skipped: boolean;
}

const EMPTY: Draft = {chosen: [], other: '', text: '', skipped: false};

/** The response `draft` gives to `question`, or `null` while it answers nothing. */
function responseOf(question: Question, draft: Draft): QuestionResponse | null {
  if (draft.skipped) return {kind: 'declined'};
  if (!question.choices?.length) return draft.text ? {kind: 'text', text: draft.text} : null;
  if (!draft.chosen.length && !draft.other) return null;
  return {kind: 'chosen', chosen: draft.chosen, ...(draft.other ? {other: draft.other} : {})};
}

interface QuestionCardProps {
  request: QuestionRequest;
  /** Whether the questions are put to this user: only then are there controls. */
  asked: boolean;
  /** Who they are put to, by name. */
  askedNames: string[];
  /** Whether this user's answer is on its way, until the agent closes the questions. */
  answered: boolean;
  /** Sends the responses, one per question - or `null` to decline them all. */
  onAnswer: (responses: QuestionResponse[] | null) => Promise<void>;
}

export const QuestionCard: React.FC<QuestionCardProps> = ({request, asked, askedNames, answered, onAnswer}) => {
  const {questions, sensitive} = request.questions;
  const [drafts, setDrafts] = useState<Draft[]>(() => questions.map(() => EMPTY));
  const [sending, setSending] = useState(false);
  const responses = questions.map((q, i) => responseOf(q, drafts[i] ?? EMPTY));
  const complete = responses.every(r => r !== null);
  const editable = asked && !answered && !sending;

  const change = (index: number, change: Partial<Draft>) =>
    setDrafts(prev => prev.map((d, i) => i === index ? {...d, ...change} : d));
  const choose = (index: number, question: Question, label: string) => {
    const chosen = drafts[index]?.chosen ?? [];
    const next = question.multi_select
      ? (chosen.includes(label) ? chosen.filter(l => l !== label) : [...chosen, label])
      : (chosen.includes(label) ? [] : [label]);
    change(index, {chosen: next});
  };
  const send = async (answer: QuestionResponse[] | null) => {
    setSending(true);
    try {
      await onAnswer(answer);
    } finally {
      setSending(false);
    }
  };
  const input = "w-full text-sm px-2 py-1 rounded-md border border-gray-300 dark:border-gray-600 bg-white dark:bg-gray-900 disabled:opacity-60";

  return (
    <div className="bg-white dark:bg-gray-800 rounded-lg border border-blue-200 dark:border-blue-900/50 shadow-xs p-4 space-y-4">
      <div className="flex items-center justify-between text-xs text-gray-500">
        <span className="font-mono font-bold">{request.tool_call.function_name}</span>
        {sensitive && (
          <span className="flex items-center gap-1" title="The answers go to the tool alone">
            <Lock size={12} /> sensitive
          </span>
        )}
      </div>
      {questions.map((question, index) => {
        const draft = drafts[index] ?? EMPTY;
        const disabled = !editable || draft.skipped;
        return (
          <fieldset key={index} className="space-y-2" disabled={!editable}>
            <legend className="text-sm">
              <span className="text-[11px] uppercase tracking-wider text-gray-500 mr-2">{question.header}</span>
              <span className="font-medium whitespace-pre-wrap">{question.question}</span>
            </legend>
            {question.choices?.length ? (
              <>
                <div className="flex flex-wrap gap-2" role="group" aria-label={question.header}>
                  {question.choices.map(choice => (
                    <button
                      key={choice.label}
                      type="button"
                      disabled={disabled}
                      aria-pressed={draft.chosen.includes(choice.label)}
                      onClick={() => choose(index, question, choice.label)}
                      title={choice.description}
                      className={cn(
                        "text-sm px-3 py-1 rounded-md border transition-colors disabled:opacity-60",
                        draft.chosen.includes(choice.label)
                          ? "bg-blue-600 border-blue-600 text-white"
                          : "border-gray-300 dark:border-gray-600 hover:bg-gray-100 dark:hover:bg-gray-700",
                      )}
                    >
                      {choice.label}
                      {choice.recommended && <span className="ml-1 text-[11px] opacity-75">(recommended)</span>}
                    </button>
                  ))}
                </div>
                {question.choices.some(c => c.description) && (
                  <ul className="text-xs text-gray-500 dark:text-gray-400 space-y-0.5">
                    {question.choices.filter(c => c.description).map(c => <li key={c.label}><span className="font-medium">{c.label}:</span> {c.description}</li>)}
                  </ul>
                )}
                {asked && (
                  <input
                    type={sensitive ? 'password' : 'text'}
                    className={input}
                    placeholder={question.multi_select ? 'Other, besides or instead' : 'Other'}
                    value={draft.other}
                    disabled={disabled}
                    onChange={e => change(index, {other: e.target.value})}
                    aria-label={`${question.header}: other`}
                  />
                )}
              </>
            ) : asked && (
              sensitive ? (
                <input type="password" className={input} value={draft.text} disabled={disabled}
                  onChange={e => change(index, {text: e.target.value})} aria-label={question.header} autoComplete="off" />
              ) : (
                <textarea className={input} rows={2} value={draft.text} disabled={disabled}
                  onChange={e => change(index, {text: e.target.value})} aria-label={question.header} />
              )
            )}
            {asked && questions.length > 1 && (
              <label className="flex items-center gap-1 text-xs text-gray-500">
                <input type="checkbox" checked={draft.skipped} disabled={!editable} onChange={e => change(index, {skipped: e.target.checked})} />
                Skip this question
              </label>
            )}
          </fieldset>
        );
      })}
      {!asked ? (
        <p className="text-sm text-gray-500 dark:text-gray-400 text-center">Waiting for {askedNames.join(', ') || 'whoever was asked'} to answer.</p>
      ) : answered ? (
        <p className="text-sm text-gray-500 dark:text-gray-400 text-center">Answered.</p>
      ) : (
        <>
          <p className="text-xs text-gray-500 dark:text-gray-400 flex items-center gap-1">
            <HelpCircle size={12} />
            {sensitive
              ? `Your answers go to the tool ${request.tool_call.function_name} alone; the agent does not see them.`
              : 'Your answers go to the agent. Writing a message instead declines the questions.'}
          </p>
          <div className="flex gap-2">
            <button
              type="button"
              disabled={!complete || sending}
              onClick={() => void send(responses as QuestionResponse[])}
              className="flex-1 bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white text-sm py-2 rounded-md transition-colors"
            >
              Answer
            </button>
            <button
              type="button"
              disabled={sending}
              onClick={() => void send(null)}
              className="flex-1 bg-gray-200 hover:bg-gray-300 dark:bg-gray-700 dark:hover:bg-gray-600 text-sm py-2 rounded-md transition-colors"
            >
              Decline
            </button>
          </div>
        </>
      )}
    </div>
  );
};
