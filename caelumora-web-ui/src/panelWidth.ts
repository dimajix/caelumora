// The width of the panels beside the chat (issue 231): the sidebar, the
// participants of a channel, and the panel showing an agent's context, history or
// tools. Each can be made wider or narrower; the width is a convenience of
// this browser, kept in local storage, since the right one depends on the
// screen.

import {useEffect, useState} from 'react';

/** The panels whose width is kept - the context, history and tools panels share one. */
export type PanelKind = 'sidebar' | 'participants' | 'detail';

/** Each panel's width in pixels: as it starts, and how narrow and wide it may be. */
const PANELS: Record<PanelKind, {initial: number; min: number; max: number}> = {
  sidebar: {initial: 256, min: 180, max: 480},
  participants: {initial: 256, min: 200, max: 480},
  detail: {initial: 448, min: 320, max: 960},
};

/** How much of the window one panel may take at most, so that the chat keeps room. */
const MAX_SHARE = 0.4;

const key = (kind: PanelKind) => `caelumora.panelWidth.${kind}`;

function load(kind: PanelKind): number | null {
  try {
    const stored = Number(localStorage.getItem(key(kind)));
    return Number.isFinite(stored) && stored > 0 ? stored : null;
  } catch {
    return null;
  }
}

function save(kind: PanelKind, width: number | null) {
  try {
    if (width === null) localStorage.removeItem(key(kind));
    else localStorage.setItem(key(kind), String(Math.round(width)));
  } catch {
    // No local storage (private mode, blocked): the width is then not kept.
  }
}

/** A panel's width, and how it is changed. */
export interface PanelWidth {
  /** The width now, within `min` and `max`. */
  width: number;
  min: number;
  /** The widest it may be in this window. */
  max: number;
  /**
   * Changes the width, within the bounds; with `keep` it is kept for the
   * next time, without - while it is dragged - not yet.
   */
  set: (width: number, keep?: boolean) => void;
  /** Back to the width it starts with, kept so. */
  reset: () => void;
}

/** The width of the panel `kind`: as kept in this browser, or as it starts. */
export function usePanelWidth(kind: PanelKind): PanelWidth {
  const {initial, min, max: widest} = PANELS[kind];
  const [chosen, setChosen] = useState<number | null>(() => load(kind));
  const [windowWidth, setWindowWidth] = useState(() => window.innerWidth);

  useEffect(() => {
    const resized = () => setWindowWidth(window.innerWidth);
    window.addEventListener('resize', resized);
    return () => window.removeEventListener('resize', resized);
  }, []);

  const max = Math.max(min, Math.min(widest, Math.floor(windowWidth * MAX_SHARE)));
  const bounded = (width: number) => Math.max(min, Math.min(max, width));
  const width = bounded(chosen ?? initial);

  return {
    width,
    min,
    max,
    set: (next, keep = false) => {
      setChosen(bounded(next));
      if (keep) save(kind, bounded(next));
    },
    reset: () => {
      setChosen(null);
      save(kind, null);
    },
  };
}
