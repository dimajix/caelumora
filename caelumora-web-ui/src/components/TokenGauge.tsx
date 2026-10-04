// How full an agent's context is, and what its model calls in the channel
// used (issue 209), as the member list shows it.

import React from 'react';
import type {TokenUsage} from '../types';
import {fillLevel, formatTokensShort, tokenUsageGroups} from '../messages';
import {HoverCard} from './HoverCard';
import {UsageStats} from './UsageStats';

/** From this share of its context on, an agent's gauge warns, and from the second it alarms. */
const WARN_AT = 0.75;
const ALARM_AT = 0.9;

/**
 * A bar of the last call's tokens against the context size - its numbers
 * and what all calls used in a hover card (issue 219). Without a known
 * size, the tokens alone.
 */
export const TokenGauge: React.FC<{usage: TokenUsage}> = ({usage}) => {
  const level = fillLevel(usage.last_call_tokens, usage.context_size);
  const card = <UsageStats groups={tokenUsageGroups(usage)} />;
  if (level === null) {
    return (
      <HoverCard content={card} className="block text-[11px] text-gray-400 dark:text-gray-500">
        {formatTokensShort(usage.last_call_tokens)} tokens
      </HoverCard>
    );
  }
  const color = level >= ALARM_AT ? 'bg-red-500' : level >= WARN_AT ? 'bg-amber-500' : 'bg-green-500';
  return (
    <HoverCard content={card} className="flex items-center gap-2">
      <div
        className="flex-1 h-1.5 rounded-full bg-gray-200 dark:bg-gray-700 overflow-hidden"
        role="meter"
        aria-label="How full the context is"
        aria-valuemin={0}
        aria-valuemax={usage.context_size}
        aria-valuenow={usage.last_call_tokens}
      >
        <div className={`h-full ${color}`} style={{width: `${Math.round(level * 100)}%`}} />
      </div>
      <span className="text-[11px] font-mono text-gray-400 dark:text-gray-500">{Math.round(level * 100)}%</span>
    </HoverCard>
  );
};
