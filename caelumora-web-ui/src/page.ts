// What is shown in the place of a channel - a workspace (issue 253), a skill
// (issue 22), an artifact (issue 291) or a team (issue 286) -, opened from
// the sidebar or from what the chat links to.

/** What kind of thing a page shows. */
export type PageKind = 'workspace' | 'skill' | 'artifact' | 'group';

/** The page of the thing `id`, of kind `kind`. */
export interface Page {
  kind: PageKind;
  id: string;
}

/** Whether `page` is the page of the thing `id`, of kind `kind`. */
export function isPageOf(page: Page | null, kind: PageKind, id: string): boolean {
  return page !== null && page.kind === kind && page.id === id;
}
