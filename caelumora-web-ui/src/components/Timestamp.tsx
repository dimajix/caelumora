// A point in time, as the UI shows it.

import React from 'react';
import {cn} from '../classNames';
import {formatTimestamp} from '../messages';

/**
 * When something was said or happened - a message, a note, an attachment, an entry of a history -:
 * date and time, the full form on hover (issue 43).
 */
export const Timestamp: React.FC<{timestamp?: string; className?: string}> = ({timestamp, className}) => {
  const time = formatTimestamp(timestamp);
  if (!time) return null;
  return (
    <time dateTime={timestamp} title={time.full} className={cn("normal-case tracking-normal font-normal", className)}>
      {time.short}
    </time>
  );
};
