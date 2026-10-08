// The steps of a plan an agent works through (issue 258): their answers, tool
// calls and results, folded under one toggle, as reasoning is folded.

import React, {useState} from 'react';
import {ChevronDown, ChevronRight, ListChecks} from 'lucide-react';

interface PlanStepsProps {
  /** Who works on the plan. */
  name: string;
  /** How many messages the steps hold. */
  count: number;
  /** Whether its turn is still under way: open then, folded once it ended. */
  underWay: boolean;
  /** The messages, as the chat renders them. */
  children: React.ReactNode;
}

export const PlanSteps: React.FC<PlanStepsProps> = ({name, count, underWay, children}) => {
  // What the user chose, if they did: it outweighs whether the turn runs.
  const [chosen, setChosen] = useState<boolean | null>(null);
  const open = chosen ?? underWay;
  return (
    // As wide as the chat: the messages in it are as wide as every other.
    <div>
      <button
        type="button"
        onClick={() => setChosen(!open)}
        aria-expanded={open}
        className="flex items-center gap-1 text-xs text-gray-600 dark:text-gray-300 opacity-80 hover:opacity-100 focus:opacity-100"
      >
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <ListChecks size={13} />
        {underWay ? (
          <span className="inline-flex items-center gap-1.5">
            {name} works on the plan
            <span className="w-1.5 h-1.5 bg-current rounded-full animate-pulse" />
          </span>
        ) : (
          <span>{name} worked on the plan · {count} {count === 1 ? 'message' : 'messages'}</span>
        )}
      </button>
      {open && (
        <div className="mt-2 pl-3 border-l-2 border-gray-200 dark:border-gray-600 space-y-4">
          {children}
        </div>
      )}
    </div>
  );
};
