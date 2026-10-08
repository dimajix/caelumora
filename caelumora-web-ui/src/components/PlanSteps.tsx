// A step of a plan an agent works through (issues 258, 263): its answers, tool calls and results,
// and the answer that streams in, folded under one toggle headed by the step, as reasoning is
// folded.

import React, {useState} from 'react';
import {ChevronDown, ChevronRight, ListChecks} from 'lucide-react';
import type {PlanStep} from '../types';

interface PlanStepsProps {
  /** Who works on the plan. */
  name: string;
  /** The step, or `null` if it is not known: the messages are then headed by the plan alone. */
  step: PlanStep | null;
  /** How many messages the step holds. */
  count: number;
  /** Whether it is the step its turn is in now: open then, folded once the turn moved on. */
  underWay: boolean;
  /** Whether an answer of the step streams in, shown after its messages - also before it has any. */
  streaming: boolean;
  /** The messages, and the answer that streams in, as the chat renders them. */
  children: React.ReactNode;
}

export const PlanSteps: React.FC<PlanStepsProps> = ({name, step, count, underWay, streaming, children}) => {
  // What the user chose, if they did: it outweighs whether the step runs.
  const [chosen, setChosen] = useState<boolean | null>(null);
  const open = chosen ?? underWay;
  const which = step ? `step ${step.step_index + 1} of ${step.total_steps}` : 'the plan';
  return (
    // As wide as the chat: the messages in it are as wide as every other.
    <div>
      <button
        type="button"
        onClick={() => setChosen(!open)}
        aria-expanded={open}
        className="flex items-start gap-1 text-left text-xs text-gray-600 dark:text-gray-300 opacity-80 hover:opacity-100 focus:opacity-100"
      >
        <span className="flex items-center gap-1 shrink-0 mt-px">
          {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
          <ListChecks size={13} />
        </span>
        <span>
          {name} {underWay ? 'works' : 'worked'} on {which}
          {step && <span className="font-medium"> · {step.description}</span>}
          {underWay
            ? <span className="inline-block ml-1.5 w-1.5 h-1.5 bg-current rounded-full animate-pulse align-middle" />
            : <span className="opacity-70"> · {count} {count === 1 ? 'message' : 'messages'}</span>}
        </span>
      </button>
      {open && (count > 0 || streaming) && (
        <div className="mt-2 pl-3 border-l-2 border-gray-200 dark:border-gray-600 space-y-4">
          {children}
        </div>
      )}
    </div>
  );
};
