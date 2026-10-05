// Placing something that floats over the page next to what opened it - a
// hover card, a menu (issues 219, 230): it is put in the page's body, so that
// no scrolling container cuts it off, and kept inside the window.

import {type RefObject, useEffectEvent, useLayoutEffect, useRef, useState} from 'react';

/** How far it keeps from what opened it and from the window's edges. */
const GAP = 6;
const MARGIN = 8;

export interface Placement {
  /** Where it goes if there is room: above or below its trigger. */
  prefer: 'above' | 'below';
  /** Which edge of the trigger it is aligned with: its left or its right. */
  align: 'start' | 'end';
}

/**
 * Where to put what floats while it is `open`: the refs to attach to the
 * `trigger` and to the `floating` element, and its `position` - next to the
 * trigger as `placement` says, on the other side where there is no room, and
 * always inside the window; `null` until it is rendered and its size is
 * known. Where the trigger is no longer - what holds it scrolled, the
 * window changed - `onGone` is called.
 */
export function useFloating<T extends HTMLElement, F extends HTMLElement>(
  open: boolean,
  placement: Placement,
  onGone: () => void,
): {trigger: RefObject<T | null>; floating: RefObject<F | null>; position: {left: number; top: number} | null} {
  const trigger = useRef<T>(null);
  const floating = useRef<F>(null);
  const [position, setPosition] = useState<{left: number; top: number} | null>(null);
  const {prefer, align} = placement;

  // Placed when it opens, and again whenever its size changes - an entry
  // asking for a confirmation is wider.
  useLayoutEffect(() => {
    const element = floating.current;
    if (!open || !element) return;
    const place = () => {
      if (!trigger.current) return;
      const anchor = trigger.current.getBoundingClientRect();
      const {width, height} = element.getBoundingClientRect();
      const above = anchor.top - GAP - height;
      const below = anchor.bottom + GAP;
      const fitsBelow = below + height <= window.innerHeight - MARGIN;
      const top = prefer === 'above'
        ? (above >= MARGIN ? above : Math.min(below, window.innerHeight - MARGIN - height))
        : (fitsBelow || above < MARGIN ? Math.min(below, window.innerHeight - MARGIN - height) : above);
      const wanted = align === 'start' ? anchor.left : anchor.right - width;
      const left = Math.max(MARGIN, Math.min(wanted, window.innerWidth - MARGIN - width));
      setPosition({left, top: Math.max(MARGIN, top)});
    };
    place();
    const observer = new ResizeObserver(place);
    observer.observe(element);
    return () => {
      observer.disconnect();
      // Not shown where it was when it opens again.
      setPosition(null);
    };
  }, [open, prefer, align]);

  const gone = useEffectEvent(onGone);
  useLayoutEffect(() => {
    if (!open) return;
    const away = () => gone();
    // Only what moves the trigger: a list beside it scrolling - a chat
    // receiving messages - leaves it where it is.
    const scrolled = (e: Event) => {
      if (e.target instanceof Node && trigger.current && e.target.contains(trigger.current)) gone();
    };
    window.addEventListener('scroll', scrolled, true);
    window.addEventListener('resize', away);
    return () => {
      window.removeEventListener('scroll', scrolled, true);
      window.removeEventListener('resize', away);
    };
  }, [open]);

  return {trigger, floating, position: open ? position : null};
}
