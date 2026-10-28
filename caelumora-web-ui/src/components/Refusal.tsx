// How a dialog shows a refusal (issue 481), as `reportUnlessRefused` hands it
// over: drawn for a dialog, which is dark whatever the theme.

import React from 'react';

/** What the server refused of a dialog's request, shown under its fields; nothing for `null`. */
export const Refusal: React.FC<{message: string | null}> = ({message}) =>
  message ? <div role="alert" className="text-xs text-red-400">{message}</div> : null;
