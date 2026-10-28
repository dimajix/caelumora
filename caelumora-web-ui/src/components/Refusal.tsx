// How a refusal is shown (issue 481), as `reportUnlessRefused` hands it over: drawn for a dialog or
// the sidebar, which are dark whatever the theme, or for a page, which follows it (issue 479).

import React from 'react';

const LOOKS = {
  dialog: 'text-xs text-red-400',
  page: 'text-xs text-red-600 dark:text-red-400',
  sidebar: 'text-xs text-red-400',
};

interface RefusalProps {
  /** What was refused; nothing is shown for `null`. */
  message: string | null;
  /** Where it is shown; a dialog if not given. */
  on?: keyof typeof LOOKS;
}

/** What the server refused of a request, shown under the fields it is about; nothing for `null`. */
export const Refusal: React.FC<RefusalProps> = ({message, on = 'dialog'}) =>
  message ? <div role="alert" className={LOOKS[on]}>{message}</div> : null;
