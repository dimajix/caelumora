// The entries of a list in the sidebar of what opens as a page in the place
// of a channel (`PageList`): loaded, loaded again on an event about one of
// them, and the page of one no longer listed closed.

import {useCallback, useEffect, useState} from 'react';
import type {ScopeKind} from './types';
import {reportError} from './errors';
import type {Page, PageKind} from './page';

/** What a list shows of each entry. */
export interface Listed {
  id: string;
  name: string;
  scope: ScopeKind;
  description?: string;
}

/** The entries of a list, as `usePageList` keeps them. */
export interface PageListState<T extends Listed> {
  kind: PageKind;
  /** What the entries are, in the plural - "workspaces". */
  what: string;
  items: T[];
  /** Adds one made here at once; the event that follows finds it there. */
  add: (item: T) => void;
  /** Loads the list again - on an event about one of its entries. */
  reload: () => void;
}

/**
 * The entries of kind `kind` the user may read, loaded by `load` and again
 * on `reload`. The page of one the list no longer has is gone
 * (`onPageGone`): deleted, here or elsewhere, or no longer readable.
 */
export function usePageList<T extends Listed>(
  kind: PageKind,
  what: string,
  load: () => Promise<T[]>,
  page: Page | null,
  onPageGone: (kind: PageKind, id: string) => void,
): PageListState<T> {
  const [items, setItems] = useState<T[]>([]);
  const [version, setVersion] = useState(0);

  useEffect(() => {
    let current = true;
    load()
      .then(found => { if (current) setItems(found); })
      .catch(error => reportError(`Failed to load the ${what}`, error));
    return () => { current = false; };
    // `load` is a new function on every render; what it loads changes with `version`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [version]);

  useEffect(() => {
    if (page?.kind === kind && version > 0 && !items.some(item => item.id === page.id)) {
      onPageGone(kind, page.id);
    }
  }, [items, page, kind, version, onPageGone]);

  const add = useCallback((item: T) => setItems(prev => (prev.some(i => i.id === item.id) ? prev : [...prev, item])), []);
  const reload = useCallback(() => setVersion(v => v + 1), []);
  return {kind, what, items, add, reload};
}
