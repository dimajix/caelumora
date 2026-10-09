// The edge between a panel and the chat (issue 231): dragged with the mouse,
// a finger or a pen, or moved with the arrow keys, it makes the panel wider or
// narrower; a double click returns it to the width it starts with. It sits
// beside the panel, not in it, so that it does not scroll with what the
// panel holds.

import React, {useRef} from 'react';
import {cn} from '../classNames';
import type {PanelWidth} from '../panelWidth';

/** How far an arrow key moves the edge, and with Shift held. */
const STEP = 16;
const LARGE_STEP = 64;

interface ResizeHandleProps {
  panel: PanelWidth;
  /** Which side of the handle the panel is on. */
  panelSide: 'left' | 'right';
  /** What is resized, for screen readers, e.g. `Resize the sidebar`. */
  label: string;
  /** Classes of the handle, e.g. to hide it where the panel is hidden. */
  className?: string;
}

export const ResizeHandle: React.FC<ResizeHandleProps> = ({panel, panelSide, label, className}) => {
  // Moving right widens a panel on the left, narrows one on the right.
  const direction = panelSide === 'left' ? 1 : -1;
  const drag = useRef<{x: number; width: number} | null>(null);

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    // Followed even where the pointer leaves the handle; where it cannot be
    // captured, as long as it stays on it.
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // The pointer is gone already.
    }
    drag.current = {x: e.clientX, width: panel.width};
  };

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    panel.set(drag.current.width + (e.clientX - drag.current.x) * direction);
  };

  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    panel.set(drag.current.width + (e.clientX - drag.current.x) * direction, true);
    drag.current = null;
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const step = e.shiftKey ? LARGE_STEP : STEP;
    const by = (delta: number) => {
      e.preventDefault();
      panel.set(panel.width + delta, true);
    };
    switch (e.key) {
      case 'ArrowRight': by(step * direction); break;
      case 'ArrowLeft': by(-step * direction); break;
      case 'Home': by(panel.min - panel.width); break;
      case 'End': by(panel.max - panel.width); break;
      case 'Enter': e.preventDefault(); panel.reset(); break;
    }
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={panel.width}
      aria-valuemin={panel.min}
      aria-valuemax={panel.max}
      tabIndex={0}
      title={`${label} - double click for its usual width`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => { drag.current = null; }}
      onDoubleClick={panel.reset}
      onKeyDown={onKeyDown}
      // Over the panel's border, without taking room from it or the chat.
      className={cn(
        'relative z-10 -mx-1 w-2 shrink-0 cursor-col-resize touch-none select-none',
        'after:absolute after:inset-y-0 after:left-1/2 after:w-0.5 after:-translate-x-1/2 after:transition-colors',
        'hover:after:bg-blue-500/60 focus-visible:outline-hidden focus-visible:after:bg-blue-500 active:after:bg-blue-500',
        className,
      )}
    />
  );
};
