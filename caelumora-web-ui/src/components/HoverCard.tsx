// A card that opens over its trigger (issue 219): on hover, on keyboard
// focus, and on tap, where there is no hover. It is placed in the page's
// body, so that no scrolling container around the trigger cuts it off, as
// wide as its content, above the trigger - below where there is no room - and
// always inside the window.

import React, {useId, useState} from 'react';
import {createPortal} from 'react-dom';
import {cn} from '../classNames';
import {useFloating} from './floating';

interface HoverCardProps {
  /** What the card is opened from; focusable unless it already holds something that is. */
  children: React.ReactNode;
  /** What the card shows. */
  content: React.ReactNode;
  /** Classes of the element around the trigger, e.g. to make it a block. */
  className?: string;
  /** Whether the trigger can be focused by itself; `false` if it is a button already. */
  focusable?: boolean;
}

const PLACEMENT = {prefer: 'above', align: 'start'} as const;

export const HoverCard: React.FC<HoverCardProps> = ({children, content, className, focusable = true}) => {
  const id = useId();
  const [open, setOpen] = useState(false);
  const close = () => setOpen(false);
  // Where the trigger is no longer, the card is not either.
  const {trigger, floating: card, position} = useFloating<HTMLSpanElement, HTMLDivElement>(open, PLACEMENT, close);

  return (
    <span
      ref={trigger}
      className={className ?? 'inline-block'}
      tabIndex={focusable ? 0 : undefined}
      aria-describedby={open ? id : undefined}
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={close}
      onFocus={() => setOpen(true)}
      onBlur={close}
      // A tap opens it where there is no hover; tapping elsewhere closes it.
      onClick={() => setOpen(true)}
      onKeyDown={e => { if (e.key === 'Escape') close(); }}
    >
      {children}
      {open && createPortal(
        <div
          ref={card}
          id={id}
          role="tooltip"
          className={cn(
            'fixed z-50 w-max max-w-[min(24rem,calc(100vw-1rem))] rounded-lg border border-gray-200 dark:border-gray-600',
            'bg-white dark:bg-gray-800 text-gray-800 dark:text-gray-100 shadow-lg px-3 py-2 text-xs normal-case tracking-normal font-normal',
            'pointer-events-none',
            position ? 'visible' : 'invisible',
          )}
          style={position ?? {left: 0, top: 0}}
        >
          {content}
        </div>,
        document.body,
      )}
    </span>
  );
};
