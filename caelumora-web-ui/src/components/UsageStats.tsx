// Usage statistics as a hover card shows them (issues 43, 209, 219): groups
// under a heading each, labels on the left and values aligned on the right,
// never wrapped.

import React from 'react';
import type {StatGroup} from '../messages';

export const UsageStats: React.FC<{groups: StatGroup[]}> = ({groups}) => (
  <div className="space-y-2">
    {groups.map(group => (
      <div key={group.title}>
        <div className="mb-1 text-[10px] font-semibold uppercase tracking-wider text-gray-500 dark:text-gray-400">
          {group.title}
        </div>
        <dl className="grid grid-cols-[auto_auto] gap-x-4 gap-y-0.5">
          {group.rows.map(([label, value]) => (
            <React.Fragment key={label}>
              <dt className="whitespace-nowrap text-gray-500 dark:text-gray-400">{label}</dt>
              <dd className="whitespace-nowrap text-right font-mono tabular-nums">{value}</dd>
            </React.Fragment>
          ))}
        </dl>
      </div>
    ))}
  </div>
);
