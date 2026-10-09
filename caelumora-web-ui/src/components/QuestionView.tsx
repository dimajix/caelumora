// A question as the chat shows it (issues 165, 268): its header and text, and its choices one below
// the other, each a row of the full width with what it means. The same rows serve the card that
// asks - with controls - and the record of a call's answers - with the answer marked among the
// choices that were offered.

import React from 'react';
import {Circle, CircleDot, MessageCircleQuestion, Square, SquareCheck} from 'lucide-react';
import type {Answers, Question, QuestionResponse} from '../types';
import {cn} from '../classNames';

/** A question's header and its text, its line breaks kept; phrasing content, to stand in a legend. */
export const QuestionHeading: React.FC<{question: Question; compact?: boolean}> = ({question, compact}) => (
  <span className={cn('block', compact ? 'text-xs' : 'text-sm')}>
    <span className="text-[11px] uppercase tracking-wider text-gray-500 mr-2">{question.header}</span>
    <span className="font-medium whitespace-pre-wrap">{question.question}</span>
  </span>
);

interface ChoiceRowsProps {
  question: Question;
  /** The labels chosen. */
  chosen: string[];
  /** Chooses or unchooses a label; without it, the rows only show what was chosen. */
  onChoose?: (label: string) => void;
  disabled?: boolean;
  /** Smaller, as a record within a tool call. */
  compact?: boolean;
}

/**
 * A question's choices, one below the other: a circle where one is chosen, a box where several may
 * be, the label, "recommended", and below it what choosing it means. Chosen rows stand out; in a
 * record, those not chosen are muted.
 */
export const ChoiceRows: React.FC<ChoiceRowsProps> = ({question, chosen, onChoose, disabled, compact}) => {
  const multi = !!question.multi_select;
  return (
    <div role={onChoose ? (multi ? 'group' : 'radiogroup') : 'list'} aria-label={question.header} className="space-y-1">
      {(question.choices ?? []).map(choice => {
        const isChosen = chosen.includes(choice.label);
        const Marker = multi ? (isChosen ? SquareCheck : Square) : (isChosen ? CircleDot : Circle);
        const className = cn(
          'w-full flex items-start gap-2 text-left rounded-md border transition-colors',
          compact ? 'px-2 py-1' : 'px-3 py-2',
          isChosen
            ? 'border-blue-500 bg-blue-50 dark:border-blue-500/70 dark:bg-blue-900/30'
            : 'border-gray-200 dark:border-gray-700',
          onChoose && !isChosen && 'hover:bg-gray-50 dark:hover:bg-gray-700/50',
          onChoose && 'disabled:opacity-60',
          !onChoose && !isChosen && 'opacity-60',
        );
        const content = (
          <>
            <Marker size={compact ? 12 : 14} className={cn('shrink-0 mt-0.5', isChosen ? 'text-blue-600 dark:text-blue-400' : 'text-gray-400')} />
            <span className="min-w-0 flex-1">
              <span className={cn(compact ? 'text-xs' : 'text-sm', isChosen && 'font-medium')}>{choice.label}</span>
              {choice.recommended && (
                <span className="ml-2 align-middle text-[10px] uppercase tracking-wide px-1.5 py-0.5 rounded-sm bg-blue-100 text-blue-700 dark:bg-blue-900/50 dark:text-blue-300">
                  recommended
                </span>
              )}
              {choice.description && (
                <span className={cn('block text-gray-500 dark:text-gray-400', compact ? 'text-[11px]' : 'text-xs')}>{choice.description}</span>
              )}
            </span>
          </>
        );
        return onChoose ? (
          <button
            key={choice.label}
            type="button"
            role={multi ? 'checkbox' : 'radio'}
            aria-checked={isChosen}
            disabled={disabled}
            onClick={() => onChoose(choice.label)}
            className={className}
          >
            {content}
          </button>
        ) : (
          <div key={choice.label} role="listitem" aria-current={isChosen || undefined} className={className}>
            {content}
          </div>
        );
      })}
    </div>
  );
};

/** One answer as the record shows it, below its question; `declined` if the whole of them was. */
const RecordedAnswer: React.FC<{question: Question; response: QuestionResponse | undefined; declined: boolean}> = ({question, response, declined}) => {
  if (question.choices?.length) {
    const chosen = response?.kind === 'chosen' ? response.chosen ?? [] : [];
    const other = response?.kind === 'chosen' ? response.other : undefined;
    return (
      <>
        <ChoiceRows question={question} chosen={chosen} compact />
        {other && (
          <div className="px-2 py-1 rounded-md border border-blue-500 bg-blue-50 dark:border-blue-500/70 dark:bg-blue-900/30 text-xs">
            <span className="text-[11px] uppercase tracking-wider text-gray-500 mr-2">Other</span>
            <span className="whitespace-pre-wrap font-medium">{other}</span>
          </div>
        )}
        {response?.kind === 'declined' && !declined && <div className="text-[11px] italic opacity-60">Skipped</div>}
      </>
    );
  }
  if (response?.kind === 'text') {
    return <blockquote className="text-xs whitespace-pre-wrap border-l-2 border-blue-500 pl-2 py-0.5">{response.text}</blockquote>;
  }
  return declined ? null : <div className="text-[11px] italic opacity-60">Skipped</div>;
};

/**
 * The record of the questions a call asked and of their answers (issues 165, 268): who answered or
 * declined, and each question with every choice that was offered and the answer marked among them,
 * or the answer written.
 */
export const AnswersRecord: React.FC<{answers: Answers}> = ({answers}) => {
  const declined = answers.responses.every(r => r.kind === 'declined');
  return (
    <div className="space-y-3">
      <div className="text-[11px] uppercase opacity-50 flex items-center gap-1">
        <MessageCircleQuestion size={11} />
        {declined ? `Declined by ${answers.answered_by_name}` : `Answered by ${answers.answered_by_name}`}
      </div>
      {answers.questions.questions.map((question, index) => (
        <div key={index} className="space-y-1">
          <QuestionHeading question={question} compact />
          <RecordedAnswer question={question} response={answers.responses[index]} declined={declined} />
        </div>
      ))}
    </div>
  );
};
